"""Typed return models for Meta SDK RPC methods."""
from __future__ import annotations

import json
from dataclasses import dataclass, field
from enum import Enum
from typing import Any

from .identity_first_models import WakeRefusal


def _coerce_int(value: Any, default: int = 0) -> int:
    """Best-effort int coercion for wire-typed dataclass fields.

    The wire schema documents these fields as JSON numbers, but stubs,
    older gateways, and test fixtures sometimes deliver `None`, `"15"`,
    or floats. Pre-fix the SDK's `from_dict` parsers passed these
    through untouched, breaking downstream arithmetic far from the
    parse site. The fallback is `default` (typically `0`).
    """
    if value is None:
        return default
    if isinstance(value, bool):
        return int(value)
    if isinstance(value, int):
        return value
    try:
        return int(value)
    except (TypeError, ValueError):
        return default


def _require_non_empty_string(data: dict[str, Any], field: str, context: str) -> str:
    value = data.get(field)
    if not isinstance(value, str) or value == "":
        raise ValueError(f"{context}.{field} must be a non-empty string")
    return value


def _require_number(data: dict[str, Any], field: str, context: str) -> int | float:
    value = data.get(field)
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"{context}.{field} must be a number")
    return value


@dataclass(frozen=True)
class StorageSlotSummary:
    """One composed storage slot's durability record (M4 census).

    Mirrors the entries of the ``storage.slots`` array on ``meta/status``
    / ``meta/capabilities``: the domain (``"sessions"``, ``"runtime"``,
    ``"blobs"``, ...), meerkat's durability vocabulary (``durability_class``
    is the wire field ``class``: ``"durable"`` / ``"scratch"``;
    ``resolution``: ``"persistent"`` / ``"declared_ephemeral"`` /
    ``"non_persistent"``), the concrete backend, whether the slot is a
    sanctioned boot-without degradation, and an optional detail note.
    Parsing is forward-tolerant: unknown fields are ignored and unknown
    vocabulary values pass through as strings.
    """

    domain: str
    durability_class: str
    resolution: str
    backend: str
    degraded: bool = False
    detail: str | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> StorageSlotSummary:
        return cls(
            domain=str(data.get("domain", "")),
            durability_class=str(data.get("class", "")),
            resolution=str(data.get("resolution", "")),
            backend=str(data.get("backend", "")),
            degraded=bool(data.get("degraded", False)),
            detail=data.get("detail"),
        )


@dataclass(frozen=True)
class StorageSummary:
    """The runtime's composition-time storage census (H1/H2 + M4).

    The ``storage`` object shared by ``meta/status``,
    ``meta/capabilities``, and ``meta/storage/doctor``:
    ``blob_durability`` / ``blob_store_persistent`` record the blob slot's
    resolution (H1), ``session_store_incremental`` the incremental
    persistence probe (H2, ``None`` when no sessions are persisted), and
    ``slots`` the per-slot durability census (M4).
    """

    blob_durability: str
    blob_store_persistent: bool
    session_store_incremental: bool | None = None
    slots: list[StorageSlotSummary] = field(default_factory=list)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> StorageSummary:
        raw_slots = data.get("slots", [])
        return cls(
            blob_durability=str(data.get("blob_durability", "")),
            blob_store_persistent=bool(data.get("blob_store_persistent", False)),
            session_store_incremental=data.get("session_store_incremental"),
            slots=[
                StorageSlotSummary.from_dict(entry)
                for entry in raw_slots
                if isinstance(entry, dict)
            ]
            if isinstance(raw_slots, list)
            else [],
        )

    def slot(self, domain: str) -> StorageSlotSummary | None:
        """The census entry for ``domain``, if the runtime reported one."""
        return next((s for s in self.slots if s.domain == domain), None)


def _storage_summary_from(data: dict[str, Any]) -> StorageSummary | None:
    raw = data.get("storage")
    return StorageSummary.from_dict(raw) if isinstance(raw, dict) else None


@dataclass(frozen=True)
class StatusResult:
    contract_version: str
    running: bool
    loaded_modules: list[str]
    storage: StorageSummary | None = None
    # ``None`` when mob announcements are disabled, their store could not be
    # read, or the gateway predates the object.
    mob_announcements: MobAnnouncementsStatus | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> StatusResult:
        announcements = data.get("mob_announcements")
        return cls(
            contract_version=data["contract_version"],
            running=data["running"],
            loaded_modules=list(data.get("loaded_modules", [])),
            storage=_storage_summary_from(data),
            mob_announcements=MobAnnouncementsStatus.from_dict(announcements)
            if isinstance(announcements, dict)
            else None,
        )


@dataclass(frozen=True)
class ProfileCapabilities:
    instance_count: int
    addressable: bool
    has_wiring: bool

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ProfileCapabilities:
        return cls(
            instance_count=int(data.get("instance_count", 0)),
            addressable=bool(data.get("addressable", True)),
            has_wiring=bool(data.get("has_wiring", False)),
        )


@dataclass(frozen=True)
class RuntimeCapabilities:
    can_spawn_members: bool
    can_send_messages: bool
    can_wire_members: bool
    can_retire_members: bool
    available_spawn_modes: list[str]
    profile_capabilities: dict[str, ProfileCapabilities] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> RuntimeCapabilities:
        pc_raw = data.get("profile_capabilities", {})
        return cls(
            can_spawn_members=bool(data.get("can_spawn_members", False)),
            can_send_messages=bool(data.get("can_send_messages", False)),
            can_wire_members=bool(data.get("can_wire_members", False)),
            can_retire_members=bool(data.get("can_retire_members", False)),
            available_spawn_modes=list(data.get("available_spawn_modes", [])),
            profile_capabilities={
                k: ProfileCapabilities.from_dict(v) for k, v in pc_raw.items()
            } if isinstance(pc_raw, dict) else {},
        )


@dataclass(frozen=True)
class CapabilitiesResult:
    contract_version: str
    methods: list[str]
    loaded_modules: list[str]
    feature_capabilities: list[str] = field(default_factory=list)
    runtime_capabilities: RuntimeCapabilities | None = None
    workgraph: bool = False
    storage: StorageSummary | None = None
    # ``None`` when mob announcements are disabled or the gateway predates
    # the object.
    mob_announcements: MobAnnouncementsCapability | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> CapabilitiesResult:
        rc_raw = data.get("runtime_capabilities")
        announcements = data.get("mob_announcements")
        return cls(
            contract_version=data["contract_version"],
            methods=list(data.get("methods", [])),
            loaded_modules=list(data.get("loaded_modules", [])),
            feature_capabilities=list(data.get("feature_capabilities", [])),
            runtime_capabilities=RuntimeCapabilities.from_dict(rc_raw) if rc_raw else None,
            workgraph=bool(data.get("workgraph", False)),
            storage=_storage_summary_from(data),
            mob_announcements=MobAnnouncementsCapability.from_dict(announcements)
            if isinstance(announcements, dict)
            else None,
        )


@dataclass(frozen=True)
class ReconcileResult:
    accepted: bool
    reconciled_modules: list[str]
    added: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ReconcileResult:
        return cls(
            accepted=data["accepted"],
            reconciled_modules=list(data.get("reconciled_modules", [])),
            added=data["added"],
        )


@dataclass(frozen=True)
class SpawnResult:
    """Result of spawning a mob member (both spec-based and module-id-based)."""
    accepted: bool
    module_id: str
    agent_identity: str | None = None
    role: str | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> SpawnResult:
        return cls(
            accepted=data["accepted"],
            module_id=data.get("module_id", ""),
            agent_identity=data.get("agent_identity"),
            role=data.get("role"),
        )


# Alias for backward compat within SDK — both spawn paths return SpawnResult
SpawnMemberResult = SpawnResult


@dataclass(frozen=True)
class KeepAliveConfig:
    interval_ms: int
    event: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> KeepAliveConfig:
        return cls(
            interval_ms=_coerce_int(data.get("interval_ms"), 0),
            event=str(data.get("event", "")),
        )


@dataclass(frozen=True)
class EventEnvelope:
    event_id: str
    source: str
    timestamp_ms: int
    event: Any

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> EventEnvelope:
        return cls(
            event_id=str(data.get("event_id", "")),
            source=str(data.get("source", "")),
            timestamp_ms=_coerce_int(data.get("timestamp_ms"), 0),
            event=data.get("event"),
        )


@dataclass(frozen=True)
class SubscribeResult:
    scope: str
    replay_from_event_id: str | None
    keep_alive: KeepAliveConfig
    keep_alive_comment: str
    event_frames: list[str]
    events: list[EventEnvelope]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> SubscribeResult:
        ka_raw = data.get("keep_alive", {})
        events_raw = data.get("events", [])
        return cls(
            scope=data["scope"],
            replay_from_event_id=data.get("replay_from_event_id"),
            keep_alive=KeepAliveConfig.from_dict(ka_raw),
            keep_alive_comment=data.get("keep_alive_comment", ""),
            event_frames=list(data.get("event_frames", [])),
            events=[EventEnvelope.from_dict(e) for e in events_raw],
        )


@dataclass(frozen=True)
class SendMessageResult:
    """Result of a send to a member/identity.

    ``session_id`` may be an empty string even when ``accepted`` is ``True``:
    in a narrow race (the target's materialized session is concurrently
    retired/rebound during the send), the server cannot resolve the delivering
    session. The send still succeeded; treat an empty ``session_id`` as
    "unknown", not as a usable session reference.
    """
    accepted: bool
    member_id: str
    session_id: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> SendMessageResult:
        return cls(
            accepted=data.get("accepted", False),
            member_id=data.get("member_id", ""),
            session_id=data.get("session_id", ""),
        )


@dataclass(frozen=True)
class RoutingResolution:
    recipient: str
    route: dict[str, Any]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> RoutingResolution:
        return cls(
            recipient=data.get("recipient", ""),
            route=dict(data.get("route", data)),
        )


@dataclass(frozen=True)
class DeliveryResult:
    delivered: bool
    delivery_id: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> DeliveryResult:
        return cls(
            delivered=data.get("delivered", False),
            delivery_id=data.get("delivery_id", ""),
        )


@dataclass(frozen=True)
class MemoryQueryResult:
    results: list[dict[str, Any]]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MemoryQueryResult:
        return cls(results=list(data.get("results", [])))


@dataclass(frozen=True)
class CallToolResult:
    """Result of calling an MCP tool on a loaded module."""
    module_id: str
    tool: str
    result: Any

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> CallToolResult:
        return cls(
            module_id=data.get("module_id", ""),
            tool=data.get("tool", ""),
            result=data.get("result"),
        )


@dataclass(frozen=True)
class RediscoverReport:
    """Report from a rediscover operation (reset + re-run discovery)."""
    spawned: list[str]
    edges: ReconcileEdgesReport

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> RediscoverReport:
        return cls(
            spawned=list(data.get("spawned", [])),
            edges=ReconcileEdgesReport.from_dict(data.get("edges", {})),
        )


@dataclass(frozen=True)
class ReconcileEdgesReport:
    """Report from dynamic edge reconciliation."""
    desired_edges: list[dict[str, Any]]
    wired_edges: list[dict[str, Any]]
    unwired_edges: list[dict[str, Any]]
    retained_edges: list[dict[str, Any]]
    preexisting_edges: list[dict[str, Any]]
    skipped_missing_members: list[dict[str, Any]]
    pruned_stale_managed_edges: list[dict[str, Any]]
    failures: list[dict[str, Any]]
    # False for a candidate launch's dry run: nothing was wired, unwired or
    # pruned, and the edge lists name what would be.
    applied: bool = True

    @property
    def is_complete(self) -> bool:
        return not self.failures and not self.skipped_missing_members

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ReconcileEdgesReport:
        return cls(
            desired_edges=list(data.get("desired_edges", [])),
            wired_edges=list(data.get("wired_edges", [])),
            unwired_edges=list(data.get("unwired_edges", [])),
            retained_edges=list(data.get("retained_edges", [])),
            preexisting_edges=list(data.get("preexisting_edges", [])),
            skipped_missing_members=list(data.get("skipped_missing_members", [])),
            pruned_stale_managed_edges=list(data.get("pruned_stale_managed_edges", [])),
            failures=list(data.get("failures", [])),
            applied=data.get("applied") is not False,
        )


@dataclass(frozen=True)
class UnifiedAgentEvent:
    """An agent event from the unified event bus.

    ``payload`` is the projected meerkat agent event (the same shape the
    per-agent SSE stream carries, for example ``error_report``/``error``/
    ``reason`` on ``run_failed``). It is part of the wire contract and must
    not be dropped; ``{}`` when the envelope carried none.
    """
    agent_id: str
    event_type: str
    payload: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class UnifiedModuleEvent:
    """A module event from the unified event bus."""
    module: str
    event_type: str
    payload: dict[str, Any] = field(default_factory=dict)


# Union of both event kinds
UnifiedEvent = UnifiedAgentEvent | UnifiedModuleEvent


def _agent_payload(value: Any) -> dict[str, Any]:
    """The agent payload as a dict; ``{}`` for an absent or non-object value."""
    return value if isinstance(value, dict) else {}


def _parse_unified_event(raw: dict[str, Any]) -> UnifiedEvent:
    """Parse a serialized UnifiedEvent.

    Rust currently emits internally tagged events, e.g.
    ``{"kind": "agent", "agent_id": "..."}``. The older externally tagged
    shape is still accepted for compatibility with pre-0.6 fixtures.
    """
    if raw.get("kind") == "agent":
        return UnifiedAgentEvent(
            agent_id=raw.get("agent_id", raw.get("agentId", "")),
            event_type=raw.get("event_type", raw.get("eventType", "")),
            payload=_agent_payload(raw.get("payload")),
        )
    if raw.get("kind") == "module":
        return UnifiedModuleEvent(
            module=raw.get("module", ""),
            event_type=raw.get("event_type", raw.get("eventType", "")),
            payload=raw.get("payload", {}),
        )
    if "Agent" in raw:
        agent = raw["Agent"]
        return UnifiedAgentEvent(
            agent_id=agent.get("agent_id", ""),
            event_type=agent.get("event_type", ""),
            payload=_agent_payload(agent.get("payload")),
        )
    if "Module" in raw:
        module = raw["Module"]
        return UnifiedModuleEvent(
            module=module.get("module", ""),
            event_type=module.get("event_type", ""),
            payload=module.get("payload", {}),
        )
    # Fallback for unknown shapes
    return UnifiedModuleEvent(module="unknown", event_type="unknown", payload=raw)


@dataclass(frozen=True)
class PersistedEvent:
    """A persisted operational event with monotonic ordering."""
    id: str
    seq: int
    timestamp_ms: int
    member_id: str | None
    event: UnifiedEvent

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> PersistedEvent:
        raw_event = data.get("event", {})
        event = _parse_unified_event(raw_event) if isinstance(raw_event, dict) else UnifiedModuleEvent(module="unknown", event_type="unknown", payload={})
        return cls(
            id=data["id"],
            seq=data["seq"],
            timestamp_ms=data["timestamp_ms"],
            member_id=data.get("member_id"),
            event=event,
        )


@dataclass
class EventQuery:
    """Query parameters for historical event retrieval.

    ``after_seq`` is the pagination cursor — pass the highest ``seq`` /
    ``cursor`` you have seen to receive only strictly-newer events.
    ``mob_id``, ``run_id``, ``step_id``, and ``identity`` filter the
    structural mob-events surface (``meta/mob_events/query``).
    """
    since_ms: int | None = None
    until_ms: int | None = None
    member_id: str | None = None
    identity: str | None = None
    mob_id: str | None = None
    run_id: str | None = None
    step_id: str | None = None
    event_types: list[str] = field(default_factory=list)
    limit: int | None = None
    after_seq: int | None = None

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {}
        if self.since_ms is not None:
            d["since_ms"] = self.since_ms
        if self.until_ms is not None:
            d["until_ms"] = self.until_ms
        if self.member_id is not None:
            d["member_id"] = self.member_id
        if self.identity is not None:
            d["identity"] = self.identity
        if self.mob_id is not None:
            d["mob_id"] = self.mob_id
        if self.run_id is not None:
            d["run_id"] = self.run_id
        if self.step_id is not None:
            d["step_id"] = self.step_id
        if self.event_types:
            d["event_types"] = self.event_types
        if self.limit is not None:
            d["limit"] = self.limit
        if self.after_seq is not None:
            d["after_seq"] = self.after_seq
        return d


@dataclass(frozen=True)
class MobStructuralEvent:
    """A structural mob event projected from ``MobEventKind``.

    Carries flow/step/identity context that the legacy lossy
    ``UnifiedEvent::Agent`` projection discards. Use ``cursor`` as the
    ``EventQuery.after_seq`` pagination token on the next request.
    """
    event_id: str
    cursor: int
    mob_id: str
    timestamp_ms: int
    kind: str
    run_id: str | None
    step_id: str | None
    agent_identity: str | None
    mob_labels: dict[str, str]
    run_labels: dict[str, str]
    data: dict[str, Any]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobStructuralEvent:
        payload = data.get("data") if isinstance(data.get("data"), dict) else {}
        mob_labels_raw = data.get("mob_labels", {})
        run_labels_raw = data.get("run_labels", {})
        return cls(
            event_id=str(data.get("event_id", "")),
            cursor=int(data.get("cursor", 0)),
            mob_id=str(data.get("mob_id", "")),
            timestamp_ms=int(data.get("timestamp_ms", 0)),
            kind=str(data.get("kind", "")),
            run_id=data.get("run_id"),
            step_id=data.get("step_id"),
            agent_identity=data.get("agent_identity"),
            mob_labels=dict(mob_labels_raw) if isinstance(mob_labels_raw, dict) else {},
            run_labels=dict(run_labels_raw) if isinstance(run_labels_raw, dict) else {},
            data=dict(payload) if isinstance(payload, dict) else {},
        )


class MobRunStatus(str, Enum):
    """Lifecycle states for a flow run.

    Mirrors meerkat's ``MobRunStatus`` enum (snake_case wire form).
    """

    PENDING = "pending"
    RUNNING = "running"
    COMPLETED = "completed"
    FAILED = "failed"
    CANCELED = "canceled"

    @classmethod
    def parse(cls, raw: Any) -> MobRunStatus:
        if isinstance(raw, MobRunStatus):
            return raw
        if isinstance(raw, str):
            try:
                return cls(raw)
            except ValueError:
                return cls.PENDING
        return cls.PENDING


@dataclass(frozen=True)
class StepRecord:
    """Per-target step execution ledger entry.

    Mirrors meerkat's ``StepLedgerEntry``. ``status`` is a snake_case
    string (e.g. ``"dispatched"``, ``"completed"``, ``"failed"``).
    """
    step_id: str
    agent_identity: str
    status: str
    output: Any
    timestamp: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> StepRecord:
        return cls(
            step_id=str(data.get("step_id", "")),
            agent_identity=str(data.get("agent_identity", "")),
            status=str(data.get("status", "")),
            output=data.get("output"),
            timestamp=str(data.get("timestamp", "")),
        )


@dataclass(frozen=True)
class FailureRecord:
    """Flow-level failure log entry. Mirrors meerkat's ``FailureLedgerEntry``."""
    step_id: str
    reason: str
    timestamp: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> FailureRecord:
        return cls(
            step_id=str(data.get("step_id", "")),
            reason=str(data.get("reason", "")),
            timestamp=str(data.get("timestamp", "")),
        )


@dataclass(frozen=True)
class FrameRecord:
    """Per-frame kernel snapshot. Mirrors meerkat's ``FrameSnapshot``.

    The ``kernel_state`` shape is meerkat-internal flow_frame state and
    passes through as :class:`Any`.
    """
    kernel_state: Any

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> FrameRecord:
        return cls(kernel_state=data.get("kernel_state"))


@dataclass(frozen=True)
class LoopRecord:
    """Per-loop kernel snapshot. Mirrors meerkat's ``LoopSnapshot``.

    The ``kernel_state`` shape is meerkat-internal loop_iteration state
    and passes through as :class:`Any`.
    """
    kernel_state: Any

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> LoopRecord:
        return cls(kernel_state=data.get("kernel_state"))


@dataclass(frozen=True)
class LoopIterationRecord:
    """Loop-iteration → body-frame ledger entry.

    Mirrors meerkat's ``LoopIterationLedgerEntry``.
    """
    loop_instance_id: str
    iteration: int
    frame_id: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> LoopIterationRecord:
        return cls(
            loop_instance_id=str(data.get("loop_instance_id", "")),
            iteration=int(data.get("iteration", 0)),
            frame_id=str(data.get("frame_id", "")),
        )


def _role_targets_from(value: Any) -> dict[str, list[str]] | None:
    """A run's ``role_targets``: each bound role with its members.

    ``None`` for a role-wide run, and for a run read from a gateway or runtime
    that predates role targets. A gate that needs a bound run must treat
    ``None`` as not authorized.
    """
    if not isinstance(value, dict):
        return None
    return {
        str(role): [str(member) for member in members]
        for role, members in value.items()
        if isinstance(members, list)
    }


@dataclass(frozen=True)
class MobRun:
    """Persisted flow run aggregate returned by :meth:`MobHandle.list_runs`.

    Carries the full meerkat ledger projection. Meerkat-internal
    sub-shapes (``flow_state``, ``activation_params``,
    ``StepRecord.output``, ``root_step_outputs`` /
    ``loop_iteration_outputs`` value blobs, frame / loop
    ``kernel_state``) pass through as :class:`Any` rather than being
    re-typed in the SDK.
    """
    run_id: str
    mob_id: str
    flow_id: str
    status: MobRunStatus
    flow_state: Any
    activation_params: Any
    created_at: str
    completed_at: str | None
    step_ledger: list[StepRecord]
    failure_ledger: list[FailureRecord]
    frames: dict[str, FrameRecord]
    loops: dict[str, LoopRecord]
    loop_iteration_ledger: list[LoopIterationRecord]
    schema_version: int
    root_step_outputs: dict[str, Any]
    loop_iteration_outputs: dict[str, Any]
    # The members each role of a role-targeted run dispatches to, as the
    # runtime recorded them; ``None`` for a role-wide run.
    role_targets: dict[str, list[str]] | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobRun:
        step_raw = data.get("step_ledger", [])
        failure_raw = data.get("failure_ledger", [])
        frames_raw = data.get("frames", {})
        loops_raw = data.get("loops", {})
        iter_raw = data.get("loop_iteration_ledger", [])
        completed = data.get("completed_at")
        return cls(
            run_id=str(data.get("run_id", "")),
            mob_id=str(data.get("mob_id", "")),
            flow_id=str(data.get("flow_id", "")),
            status=MobRunStatus.parse(data.get("status")),
            flow_state=data.get("flow_state"),
            activation_params=data.get("activation_params"),
            created_at=str(data.get("created_at", "")),
            completed_at=str(completed) if completed is not None else None,
            step_ledger=[
                StepRecord.from_dict(entry)
                for entry in step_raw
                if isinstance(entry, dict)
            ],
            failure_ledger=[
                FailureRecord.from_dict(entry)
                for entry in failure_raw
                if isinstance(entry, dict)
            ],
            frames={
                str(key): FrameRecord.from_dict(value)
                for key, value in (frames_raw.items() if isinstance(frames_raw, dict) else [])
                if isinstance(value, dict)
            },
            loops={
                str(key): LoopRecord.from_dict(value)
                for key, value in (loops_raw.items() if isinstance(loops_raw, dict) else [])
                if isinstance(value, dict)
            },
            loop_iteration_ledger=[
                LoopIterationRecord.from_dict(entry)
                for entry in iter_raw
                if isinstance(entry, dict)
            ],
            schema_version=int(data.get("schema_version", 0)),
            root_step_outputs=dict(data.get("root_step_outputs", {}))
            if isinstance(data.get("root_step_outputs"), dict)
            else {},
            loop_iteration_outputs=dict(data.get("loop_iteration_outputs", {}))
            if isinstance(data.get("loop_iteration_outputs"), dict)
            else {},
            role_targets=_role_targets_from(data.get("role_targets")),
        )


MEMBER_STATE_ACTIVE: str = "active"
MEMBER_STATE_RETIRING: str = "retiring"
# meerkat 0.7.x emits three additional member states beyond active/retiring.
# `MobMemberStatus` is `#[non_exhaustive]`, so treat the string field as open:
# branch on the documented values but tolerate future additions.
MEMBER_STATE_BROKEN: str = "broken"
MEMBER_STATE_COMPLETED: str = "completed"
MEMBER_STATE_UNKNOWN: str = "unknown"


@dataclass(frozen=True)
class MemberSnapshot:
    """Snapshot of a mob member from the roster.

    The ``state`` field is one of :data:`MEMBER_STATE_ACTIVE`,
    :data:`MEMBER_STATE_RETIRING`, :data:`MEMBER_STATE_BROKEN`,
    :data:`MEMBER_STATE_COMPLETED`, or :data:`MEMBER_STATE_UNKNOWN`. The
    underlying ``MobMemberStatus`` is ``#[non_exhaustive]`` on the Rust side, so
    consumers should branch on the known values and tolerate future ones rather
    than assuming the set is closed.
    """
    agent_identity: str
    role: str
    state: str
    wired_to: list[str]
    labels: dict[str, str]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MemberSnapshot:
        # `.get()` with defaults (not hard-indexing) to match the TS parser's
        # graceful degradation rather than raising KeyError on a missing key.
        return cls(
            agent_identity=str(data.get("agent_identity", "")),
            role=str(data.get("role", "")),
            state=str(data.get("state", "")),
            wired_to=list(data.get("wired_to", [])),
            labels=dict(data.get("labels", {})),
        )


@dataclass(frozen=True)
class MemberReloadResult:
    """Result of ``meta/reload_member`` (non-destructive cold reload).

    ``disposition`` is ``"discarded"`` (the live registration was replaced by
    a re-materialization of the SAME durable session), ``"not_degraded"`` (a
    success no-op; only produced once meerkat exposes durability state),
    ``"reattached"`` (the registration had lost its runtime attachment and was
    re-attached to the same durable session) or
    ``"not_current"`` (nothing live to reload). ``session_id`` and
    ``generation`` are the identity's binding AFTER the reload; a reload never
    advances the generation. Tolerate future disposition strings.
    """
    reloaded: bool
    disposition: str
    session_id: str | None
    generation: int | None
    identity: str | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MemberReloadResult:
        generation = data.get("generation")
        return cls(
            reloaded=bool(data.get("reloaded", False)),
            disposition=str(data.get("disposition", "")),
            session_id=(str(data["session_id"]) if data.get("session_id") is not None else None),
            generation=(int(generation) if generation is not None else None),
            identity=(str(data["identity"]) if data.get("identity") is not None else None),
        )


@dataclass(frozen=True)
class MemberHealth:
    """Result of ``meta/member_health``: lifecycle and delivery health read
    in-process (it answers while the mob actor loop is stalled).

    ``actor_loop`` is the shared probe verdict ``{"state": "live" | "stalled"
    | "terminated" | "unobserved", ...}``; ``open_stall_id`` is set while a
    stall is open. ``last_delivery_error`` is ``{"class", "detail",
    "at_unix_ms"}`` or ``None``. ``durability`` is ``"ready"`` or
    ``{"reload_required": {"operation", "reason"}}`` once the gateway can
    observe it, and ``None`` before that. ``generation`` is Core's member
    generation.
    """
    identity: str
    member_id: str | None
    state: str
    materialization_in_flight: bool
    session_id: str | None
    generation: int | None
    actor_loop: dict[str, Any]
    open_stall_id: int | None
    last_delivery_error: dict[str, Any] | None
    durability: Any | None
    bootstrap_state: str | None = None
    #: ``{"outcome": "discarded" | "reattached" | "not_degraded" | "not_current" | "refused" |
    #: "timed_out" | "failed", "detail"?, "at_unix_ms"}`` for the most recent
    #: reload attempt; ``refused`` carries meerkat's reason (store not healthy).
    last_reload: dict[str, Any] | None = None
    #: The identity's last refused wake, as on ``IdentityStatus``; ``None``
    #: when its last wake was not refused.
    wake_refusal: WakeRefusal | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MemberHealth:
        generation = data.get("generation")
        actor_loop = data.get("actor_loop")
        last_error = data.get("last_delivery_error")
        last_reload = data.get("last_reload")
        wake_refusal = data.get("wake_refusal")
        return cls(
            identity=str(data.get("identity", "")),
            member_id=(str(data["member_id"]) if data.get("member_id") is not None else None),
            state=str(data.get("state", "")),
            materialization_in_flight=bool(data.get("materialization_in_flight", False)),
            session_id=(str(data["session_id"]) if data.get("session_id") is not None else None),
            generation=(int(generation) if generation is not None else None),
            actor_loop=(dict(actor_loop) if isinstance(actor_loop, dict) else {"state": "unobserved"}),
            open_stall_id=(
                int(data["open_stall_id"]) if data.get("open_stall_id") is not None else None
            ),
            last_delivery_error=(dict(last_error) if isinstance(last_error, dict) else None),
            durability=data.get("durability"),
            bootstrap_state=(
                str(data["bootstrap_state"]) if data.get("bootstrap_state") is not None else None
            ),
            last_reload=(dict(last_reload) if isinstance(last_reload, dict) else None),
            wake_refusal=(
                WakeRefusal.from_dict(wake_refusal) if isinstance(wake_refusal, dict) else None
            ),
        )


@dataclass(frozen=True)
class RuntimeRouteResult:
    """A runtime route entry."""
    route_key: str
    recipient: str
    channel: str | None
    sink: str
    target_module: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> RuntimeRouteResult:
        return cls(
            route_key=data["route_key"],
            recipient=data["recipient"],
            channel=data.get("channel"),
            sink=data["sink"],
            target_module=data["target_module"],
        )


@dataclass(frozen=True)
class DeliveryHistoryResult:
    """Result of a delivery history query."""
    deliveries: list[dict[str, Any]]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> DeliveryHistoryResult:
        return cls(deliveries=list(data.get("deliveries", [])))


@dataclass(frozen=True)
class GatingEvaluateResult:
    """Result of a gating evaluation."""
    action_id: str
    action: str
    actor_id: str
    risk_tier: str | None
    outcome: str
    pending_id: str | None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> GatingEvaluateResult:
        return cls(
            action_id=data["action_id"],
            action=data["action"],
            actor_id=data["actor_id"],
            risk_tier=data.get("risk_tier"),
            outcome=data["outcome"],
            pending_id=data.get("pending_id"),
        )


@dataclass(frozen=True)
class GatingDecisionResult:
    """Recorded gating decision; missing outcome does not imply permission."""
    pending_id: str
    action_id: str
    decision: str
    approver_id: str | None = None
    outcome: str | None = None
    decided_at_ms: int | None = None
    reason: str | None = None
    next_pending_id: str | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> GatingDecisionResult:
        return cls(
            pending_id=data["pending_id"],
            action_id=data["action_id"],
            decision=data["decision"],
            approver_id=data.get("approver_id"),
            outcome=data.get("outcome"),
            decided_at_ms=data.get("decided_at_ms"),
            reason=data.get("reason"),
            next_pending_id=data.get("next_pending_id"),
        )


@dataclass(frozen=True)
class GatingAuditEntry:
    """An entry in the gating audit log.

    ``pending_id`` names the pending entry the record belongs to when the owner
    recorded one. ``detail`` is the owner's event detail, preserved as
    returned; an absent or malformed detail is an empty mapping.
    """
    audit_id: str
    timestamp_ms: int
    event_type: str
    action_id: str
    actor_id: str
    risk_tier: str | None
    outcome: str
    pending_id: str | None = None
    detail: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> GatingAuditEntry:
        # `.get()` with defaults (not hard-indexing) to match the TS parser's
        # graceful degradation: a missing key from an older/edge gateway should
        # not raise KeyError where TypeScript silently defaults.
        pending_id = data.get("pending_id")
        detail = data.get("detail")
        return cls(
            audit_id=str(data.get("audit_id", "")),
            timestamp_ms=int(data.get("timestamp_ms", 0)),
            event_type=str(data.get("event_type", "")),
            action_id=str(data.get("action_id", "")),
            actor_id=str(data.get("actor_id", "")),
            risk_tier=data.get("risk_tier"),
            outcome=str(data.get("outcome", "")),
            pending_id=pending_id if isinstance(pending_id, str) and pending_id.strip() else None,
            detail=dict(detail) if isinstance(detail, dict) else {},
        )


@dataclass(frozen=True)
class GatingOrigin:
    """Where a gated action was requested from, as recorded by the owner."""
    identity: str
    conversation_id: str | None = None
    interaction_id: str | None = None

    @classmethod
    def from_dict(cls, data: Any) -> GatingOrigin | None:
        """Return the origin, or ``None`` when it is absent or has no identity."""
        if not isinstance(data, dict):
            return None

        def text(value: Any) -> str | None:
            return value if isinstance(value, str) and value.strip() else None

        identity = text(data.get("identity"))
        if identity is None:
            return None
        return cls(
            identity=identity,
            conversation_id=text(data.get("conversation_id")),
            interaction_id=text(data.get("interaction_id")),
        )


@dataclass(frozen=True)
class GatingPendingEntry:
    """A pending gating decision awaiting approval."""
    pending_id: str
    action_id: str
    action: str
    actor_id: str
    risk_tier: str | None
    created_at_ms: int
    requested_approver: str | None = None
    approval_recipient: str | None = None
    approval_channel: str | None = None
    approval_route_id: str | None = None
    approval_delivery_id: str | None = None
    deadline_at_ms: int | None = None
    rationale: str | None = None
    origin: GatingOrigin | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> GatingPendingEntry:
        def text(value: Any) -> str | None:
            return value if isinstance(value, str) and value.strip() else None

        deadline = data.get("deadline_at_ms")
        return cls(
            pending_id=data["pending_id"],
            action_id=data["action_id"],
            action=data["action"],
            actor_id=data["actor_id"],
            risk_tier=data.get("risk_tier"),
            created_at_ms=data.get("created_at_ms", 0),
            requested_approver=text(data.get("requested_approver")),
            approval_recipient=text(data.get("approval_recipient")),
            approval_channel=text(data.get("approval_channel")),
            approval_route_id=text(data.get("approval_route_id")),
            approval_delivery_id=text(data.get("approval_delivery_id")),
            deadline_at_ms=deadline if type(deadline) is int and deadline >= 0 else None,
            rationale=text(data.get("rationale")),
            origin=GatingOrigin.from_dict(data.get("origin")),
        )


@dataclass(frozen=True)
class MemoryStoreInfo:
    """Information about a memory store."""
    store: str
    record_count: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MemoryStoreInfo:
        return cls(
            store=data["store"],
            record_count=data.get("record_count", 0),
        )


@dataclass(frozen=True)
class MemoryIndexResult:
    """Result of a memory index operation."""
    entity: str
    topic: str
    store: str
    assertion_id: str | None
    conflict_active: bool = False

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MemoryIndexResult:
        return cls(
            entity=data["entity"],
            topic=data["topic"],
            store=data["store"],
            assertion_id=data.get("assertion_id"),
            conflict_active=data.get("conflict_active", False),
        )


@dataclass(frozen=True)
class AgentMemoryRecord:
    """Identity-scoped durable agent memory record."""
    memory_id: str
    title: str
    body: str
    tags: list[str]
    created_at_ms: int | float
    updated_at_ms: int | float

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> AgentMemoryRecord:
        if not isinstance(data, dict):
            raise ValueError("agent_memory_record must be an object")
        memory_id = _require_non_empty_string(
            data, "memory_id", "agent_memory_record"
        )
        title = _require_non_empty_string(data, "title", "agent_memory_record")
        body = _require_non_empty_string(data, "body", "agent_memory_record")
        created_at_ms = _require_number(
            data, "created_at_ms", "agent_memory_record"
        )
        updated_at_ms = _require_number(
            data, "updated_at_ms", "agent_memory_record"
        )
        tags = data.get("tags")
        if not isinstance(tags, list) or any(not isinstance(tag, str) for tag in tags):
            raise ValueError("agent_memory_record.tags must be an array of strings")
        return cls(
            memory_id=memory_id,
            title=title,
            body=body,
            tags=list(tags),
            created_at_ms=created_at_ms,
            updated_at_ms=updated_at_ms,
        )


@dataclass(frozen=True)
class AgentMemoryRecallResult:
    """Envelope returned by meta/agent_memory/recall."""
    records: list[AgentMemoryRecord]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> AgentMemoryRecallResult:
        if not isinstance(data, dict):
            raise ValueError("agent_memory_recall_result must be an object")
        records = data.get("records")
        if not isinstance(records, list):
            raise ValueError("agent_memory_recall_result.records must be an array")
        return cls(records=[AgentMemoryRecord.from_dict(record) for record in records])


@dataclass(frozen=True)
class AgentMemoryForgetResult:
    """Result of deleting an identity-scoped durable memory record."""
    memory_id: str
    deleted: bool

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> AgentMemoryForgetResult:
        if not isinstance(data, dict):
            raise ValueError("agent_memory_forget_result must be an object")
        memory_id = _require_non_empty_string(
            data, "memory_id", "agent_memory_forget_result"
        )
        deleted = data.get("deleted")
        if not isinstance(deleted, bool):
            raise ValueError("agent_memory_forget_result.deleted must be a boolean")
        return cls(
            memory_id=memory_id,
            deleted=deleted,
        )


@dataclass(frozen=True)
class AgentMemoryUpdateResult:
    """Result of meta/agent_memory/update: the superseding record's id."""
    memory_id: str
    supersedes: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> AgentMemoryUpdateResult:
        if not isinstance(data, dict):
            raise ValueError("agent_memory_update_result must be an object")
        memory_id = _require_non_empty_string(
            data, "memory_id", "agent_memory_update_result"
        )
        supersedes = _require_non_empty_string(
            data, "supersedes", "agent_memory_update_result"
        )
        return cls(memory_id=memory_id, supersedes=supersedes)


@dataclass(frozen=True)
class AgentMemoryRecordMeta:
    """Manifest row: record metadata without the body (an index, not a dump)."""
    id: str
    kind: str
    title: str
    description: str
    age_days: int | float
    rank: int | None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> AgentMemoryRecordMeta:
        if not isinstance(data, dict):
            raise ValueError("agent_memory_record_meta must be an object")
        record_id = _require_non_empty_string(data, "id", "agent_memory_record_meta")
        kind = _require_non_empty_string(data, "kind", "agent_memory_record_meta")
        title = _require_non_empty_string(data, "title", "agent_memory_record_meta")
        description = data.get("description", "")
        if not isinstance(description, str):
            raise ValueError("agent_memory_record_meta.description must be a string")
        age_days = _require_number(data, "age_days", "agent_memory_record_meta")
        rank = data.get("rank")
        if rank is not None and not isinstance(rank, int):
            raise ValueError("agent_memory_record_meta.rank must be an integer")
        return cls(
            id=record_id,
            kind=kind,
            title=title,
            description=description,
            age_days=age_days,
            rank=rank,
        )


@dataclass(frozen=True)
class AgentMemoryManifestResult:
    """Envelope returned by meta/agent_memory/manifest."""
    records: list[AgentMemoryRecordMeta]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> AgentMemoryManifestResult:
        if not isinstance(data, dict):
            raise ValueError("agent_memory_manifest_result must be an object")
        records = data.get("records")
        if not isinstance(records, list):
            raise ValueError("agent_memory_manifest_result.records must be an array")
        return cls(
            records=[AgentMemoryRecordMeta.from_dict(record) for record in records]
        )


@dataclass(frozen=True)
class CrossMobContactEntry:
    """An entry in the cross-mob contact directory."""
    mob_id: str
    transport: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> CrossMobContactEntry:
        transport = data.get("transport", "")
        if isinstance(transport, dict):
            if "Tcp" in transport:
                transport = f"tcp://{transport['Tcp']}"
            elif "Uds" in transport:
                transport = f"uds://{transport['Uds']}"
            else:
                transport = "inproc"
        elif transport == "Inproc":
            transport = "inproc"
        return cls(mob_id=data.get("mob_id", ""), transport=transport)


@dataclass(frozen=True)
class CatalogEntry:
    """A curated model entry from the model catalog."""
    id: str
    display_name: str
    provider: str
    tier: str
    context_window: int | None = None
    max_output_tokens: int | None = None
    vision: bool = False
    image_tool_results: bool = False

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> CatalogEntry:
        profile = data.get("profile", {})
        return cls(
            id=data["id"],
            display_name=data["display_name"],
            provider=data["provider"],
            tier=data["tier"],
            context_window=data.get("context_window"),
            max_output_tokens=data.get("max_output_tokens"),
            vision=profile.get("vision", False),
            image_tool_results=profile.get("image_tool_results", False),
        )


@dataclass(frozen=True)
class ProviderDefaults:
    """Provider-level grouping with a default model."""
    provider: str
    default_model_id: str
    models: list[CatalogEntry]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ProviderDefaults:
        return cls(
            provider=data["provider"],
            default_model_id=data["default_model_id"],
            models=[CatalogEntry.from_dict(m) for m in data.get("models", [])],
        )


@dataclass(frozen=True)
class ModelsCatalogResult:
    """Result of a models/catalog RPC call."""
    models: list[CatalogEntry]
    provider_defaults: list[ProviderDefaults]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ModelsCatalogResult:
        return cls(
            models=[CatalogEntry.from_dict(m) for m in data.get("models", [])],
            provider_defaults=[
                ProviderDefaults.from_dict(p) for p in data.get("provider_defaults", [])
            ],
        )


@dataclass(frozen=True)
class ScheduleToolDefinition:
    """One of Meerkat's durable schedule tools, as ``meta/schedule/tools`` lists it.

    ``input_schema`` is the tool's JSON input schema exactly as Meerkat
    defines it; this SDK carries it without re-declaring its shape.
    """

    name: str
    description: str
    input_schema: dict[str, Any]


@dataclass(frozen=True)
class ScheduleToolsResult:
    """Result of ``meta/schedule/tools``: Meerkat's ``ScheduleToolsResult``.

    The base definitions of the ``meerkat_schedule_*`` tools a member whose
    profile sets ``tools.schedule = true`` is given. Inside that member's
    session, ``meerkat_schedule_create`` and ``meerkat_schedule_update`` also
    accept a session target of ``type: "current_session"``.
    """

    tools: list[ScheduleToolDefinition]

    @classmethod
    def from_dict(cls, data: Any) -> ScheduleToolsResult:
        """Parse the result; raise ``ValueError`` on any other shape."""
        tools = data.get("tools") if isinstance(data, dict) else None
        if not isinstance(tools, list):
            raise ValueError(f"invalid meta/schedule/tools result: {data!r}")
        parsed: list[ScheduleToolDefinition] = []
        for tool in tools:
            if (
                not isinstance(tool, dict)
                or not isinstance(tool.get("name"), str)
                or not isinstance(tool.get("description"), str)
                or not isinstance(tool.get("inputSchema"), dict)
            ):
                raise ValueError(f"invalid meta/schedule/tools result: {data!r}")
            parsed.append(
                ScheduleToolDefinition(
                    name=tool["name"],
                    description=tool["description"],
                    input_schema=dict(tool["inputSchema"]),
                )
            )
        return cls(tools=parsed)


@dataclass(frozen=True)
class MobpackToolsCatalogResult:
    """Result of a meta/tools/catalog RPC call."""
    schema_version: str
    runtime_backed: bool
    source: str
    authoring_provider: dict[str, Any]
    runtime_unavailable_reason: str | None
    tool_catalog: list[dict[str, Any]]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackToolsCatalogResult:
        return cls(
            schema_version=str(data.get("schema_version", "")),
            runtime_backed=bool(data.get("runtime_backed", False)),
            source=str(data.get("source", "")),
            authoring_provider=dict(data.get("authoring_provider", {})),
            runtime_unavailable_reason=data.get("runtime_unavailable_reason"),
            tool_catalog=list(data.get("tool_catalog", [])),
        )


@dataclass(frozen=True)
class IdentityRoutingStatusResult:
    """Model routing status from meta/identity/routing_status.

    Relays meerkat's ``SessionModelRoutingStatus`` unchanged. The override
    summaries are kept as raw dicts on purpose: re-declaring meerkat's
    ``ScopedModelOverrideSummary`` / ``SwitchTurnRequestSummary`` here would
    create a second owner of a shape this SDK only forwards.
    """

    identity: str
    session_id: str
    baseline_model: str
    effective_model: str
    #: Typed provider of the session's current LLM identity.
    #:
    #: ``None`` means the runtime machine has no hydrated session LLM identity
    #: yet (pre-hydration). It is NOT a signal to re-derive a provider from
    #: ``effective_model``: meerkat documents that re-derivation as silently
    #: wrong for models owned by a custom ``ModelRegistry``, which is the whole
    #: reason this field is carried typed rather than inferred.
    session_provider: str | None
    active_turn_override: dict[str, Any] | None
    active_operation_override: dict[str, Any] | None
    pending_switch_turn: dict[str, Any] | None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> IdentityRoutingStatusResult:
        def _opt_obj(key: str) -> dict[str, Any] | None:
            value = data.get(key)
            return dict(value) if isinstance(value, dict) else None

        provider = data.get("session_provider")
        return cls(
            identity=str(data.get("identity", "")),
            session_id=str(data.get("session_id", "")),
            baseline_model=str(data.get("baseline_model", "")),
            effective_model=str(data.get("effective_model", "")),
            session_provider=str(provider) if provider is not None else None,
            active_turn_override=_opt_obj("active_turn_override"),
            active_operation_override=_opt_obj("active_operation_override"),
            pending_switch_turn=_opt_obj("pending_switch_turn"),
        )


@dataclass(frozen=True)
class IdentityResolvedToolsResult:
    """Resolved per-identity tool surface from meta/identity/resolved_tools."""
    identity: str
    session_id: str
    tools: list[str]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> IdentityResolvedToolsResult:
        return cls(
            identity=str(data.get("identity", "")),
            session_id=str(data.get("session_id", "")),
            tools=[str(tool) for tool in data.get("tools", [])],
        )


@dataclass(frozen=True)
class MobpackSkillsCatalogResult:
    """Result of a meta/skills/catalog RPC call."""
    schema_version: str
    runtime_backed: bool
    source: str
    authoring_provider: dict[str, Any]
    runtime_unavailable_reason: str | None
    skill_realms: list[dict[str, Any]]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackSkillsCatalogResult:
        return cls(
            schema_version=str(data.get("schema_version", "")),
            runtime_backed=bool(data.get("runtime_backed", False)),
            source=str(data.get("source", "")),
            authoring_provider=dict(data.get("authoring_provider", {})),
            runtime_unavailable_reason=data.get("runtime_unavailable_reason"),
            skill_realms=list(data.get("skill_realms", [])),
        )


@dataclass(frozen=True)
class MobpackAgentDefinitionsResult:
    """Result of a meta/agent_definitions/list RPC call."""
    schema_version: str
    runtime_backed: bool
    source: str
    authoring_provider: dict[str, Any]
    runtime_unavailable_reason: str | None
    agent_definitions: list[dict[str, Any]]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackAgentDefinitionsResult:
        return cls(
            schema_version=str(data.get("schema_version", "")),
            runtime_backed=bool(data.get("runtime_backed", False)),
            source=str(data.get("source", "")),
            authoring_provider=dict(data.get("authoring_provider", {})),
            runtime_unavailable_reason=data.get("runtime_unavailable_reason"),
            agent_definitions=list(data.get("agent_definitions", [])),
        )


@dataclass(frozen=True)
class MobpackTemplatesResult:
    """Result of a meta/mobpacks/templates RPC call."""
    schema_version: str
    source: str
    authoring_provider: dict[str, Any]
    runtime_unavailable_reason: str | None
    blank_mobpack: dict[str, Any] | None
    sample_mobpacks: list[dict[str, Any]]
    sample_agent_definitions: list[dict[str, Any]]
    templates: dict[str, Any]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackTemplatesResult:
        blank = data.get("blank_mobpack")
        return cls(
            schema_version=str(data.get("schema_version", "")),
            source=str(data.get("source", "")),
            authoring_provider=dict(data.get("authoring_provider", {})),
            runtime_unavailable_reason=data.get("runtime_unavailable_reason"),
            blank_mobpack=blank if isinstance(blank, dict) else None,
            sample_mobpacks=list(data.get("sample_mobpacks", [])),
            sample_agent_definitions=list(data.get("sample_agent_definitions", [])),
            templates=dict(data.get("templates", {})),
        )


@dataclass(frozen=True)
class MobpackCatalogsResult:
    """Result of a meta/mobpacks/catalogs RPC call."""
    schema_version: str
    runtime_backed: bool
    authoring_provider: dict[str, Any]
    runtime_unavailable_reason: str | None
    sources: dict[str, Any]
    templates: dict[str, Any]
    tool_catalog: list[dict[str, Any]]
    skill_realms: list[dict[str, Any]]
    blank_mobpack: dict[str, Any] | None
    sample_mobpacks: list[dict[str, Any]]
    agent_definitions: list[dict[str, Any]]
    sample_agent_definitions: list[dict[str, Any]]
    models: list[CatalogEntry]
    provider_defaults: list[ProviderDefaults]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackCatalogsResult:
        blank = data.get("blank_mobpack")
        return cls(
            schema_version=str(data.get("schema_version", "")),
            runtime_backed=bool(data.get("runtime_backed", False)),
            authoring_provider=dict(data.get("authoring_provider", {})),
            runtime_unavailable_reason=data.get("runtime_unavailable_reason"),
            sources=dict(data.get("sources", {})),
            templates=dict(data.get("templates", {})),
            tool_catalog=list(data.get("tool_catalog", [])),
            skill_realms=list(data.get("skill_realms", [])),
            blank_mobpack=blank if isinstance(blank, dict) else None,
            sample_mobpacks=list(data.get("sample_mobpacks", [])),
            agent_definitions=list(data.get("agent_definitions", [])),
            sample_agent_definitions=list(data.get("sample_agent_definitions", [])),
            models=[CatalogEntry.from_dict(m) for m in data.get("models", [])],
            provider_defaults=[
                ProviderDefaults.from_dict(p) for p in data.get("provider_defaults", [])
            ],
        )


@dataclass(frozen=True)
class MobpackDiagnostic:
    """A single diagnostic emitted by mobpack validation."""
    severity: str
    code: str
    message: str
    path: str | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackDiagnostic:
        return cls(
            severity=str(data.get("severity", "")),
            code=str(data.get("code", "")),
            message=str(data.get("message", "")),
            path=data.get("path"),
        )


@dataclass(frozen=True)
class MobpackDisplayRow:
    """A console-style display row emitted by mobpack validation/deploy."""
    kind: str
    glyph: str
    head: str
    sub: str
    meta: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackDisplayRow:
        return cls(
            kind=str(data.get("kind", "")),
            glyph=str(data.get("glyph", "")),
            head=str(data.get("head", "")),
            sub=str(data.get("sub", "")),
            meta=str(data.get("meta", "")),
        )


@dataclass(frozen=True)
class MobpackValidationResult:
    """Result of a meta/mobpacks/validate RPC call."""
    ok: bool
    diagnostics: list[MobpackDiagnostic]
    display_rows: list[MobpackDisplayRow]
    flow_ids: list[str]
    validation_source: str
    deploy_command: str
    mob_id: str | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackValidationResult:
        return cls(
            ok=bool(data.get("ok", False)),
            diagnostics=[
                MobpackDiagnostic.from_dict(d) for d in data.get("diagnostics", [])
            ],
            display_rows=[
                MobpackDisplayRow.from_dict(r) for r in data.get("display_rows", [])
            ],
            flow_ids=list(data.get("flow_ids", [])),
            validation_source=str(data.get("validation_source", "")),
            deploy_command=str(data.get("deploy_command", "")),
            mob_id=data.get("mob_id"),
        )


@dataclass(frozen=True)
class MobpackSourceFile:
    """A rendered file inside a mobpack archive."""
    path: str
    media_type: str
    size_bytes: int
    content_base64: str
    sha256: str
    text: str | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackSourceFile:
        return cls(
            path=str(data.get("path", "")),
            media_type=str(data.get("media_type", "")),
            size_bytes=_coerce_int(data.get("size_bytes")),
            content_base64=str(data.get("content_base64", "")),
            sha256=str(data.get("sha256", "")),
            text=data.get("text"),
        )


@dataclass(frozen=True)
class MobpackSourceResult:
    """Result of a meta/mobpacks/source RPC call."""
    filename: str
    media_type: str
    mob_toml: str
    source_files: list[MobpackSourceFile]
    validation: MobpackValidationResult
    source: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackSourceResult:
        return cls(
            filename=str(data.get("filename", "")),
            media_type=str(data.get("media_type", "")),
            mob_toml=str(data.get("mob_toml", "")),
            source_files=[
                MobpackSourceFile.from_dict(f) for f in data.get("source_files", [])
            ],
            validation=MobpackValidationResult.from_dict(data.get("validation", {})),
            source=str(data.get("source", "")),
        )


@dataclass(frozen=True)
class MobpackExportResult:
    """Result of a meta/mobpacks/export RPC call."""
    filename: str
    media_type: str
    content_base64: str
    mob_toml: str
    source_files: list[MobpackSourceFile]
    validation: MobpackValidationResult

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackExportResult:
        return cls(
            filename=str(data.get("filename", "")),
            media_type=str(data.get("media_type", "")),
            content_base64=str(data.get("content_base64", "")),
            mob_toml=str(data.get("mob_toml", "")),
            source_files=[
                MobpackSourceFile.from_dict(f) for f in data.get("source_files", [])
            ],
            validation=MobpackValidationResult.from_dict(data.get("validation", {})),
        )


@dataclass(frozen=True)
class MobpackImportResult:
    """Result of a meta/mobpacks/import RPC call."""
    document: dict[str, Any]
    validation: MobpackValidationResult
    source: str
    source_label: str
    source_media_type: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackImportResult:
        return cls(
            document=dict(data.get("document", {})),
            validation=MobpackValidationResult.from_dict(data.get("validation", {})),
            source=str(data.get("source", "")),
            source_label=str(data.get("source_label", "")),
            source_media_type=str(data.get("source_media_type", "")),
        )


@dataclass(frozen=True)
class MobpackDraftRow:
    """A row from the mobpack draft registry.

    The ``document`` and ``validation`` payloads are passed through as
    permissive dicts — the mobpack document schema is opaque to the SDK.
    """
    id: str
    name: str
    version: str
    stage: str
    trigger: str
    source: str
    revision: int
    etag: str
    updated_at_unix_ms: int
    document: dict[str, Any]
    validation: dict[str, Any]
    can_undo: bool | None = None
    can_redo: bool | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackDraftRow:
        document = data.get("document")
        validation = data.get("validation")
        can_undo = data.get("can_undo")
        can_redo = data.get("can_redo")
        return cls(
            id=str(data.get("id", "")),
            name=str(data.get("name", "")),
            version=str(data.get("version", "")),
            stage=str(data.get("stage", "")),
            trigger=str(data.get("trigger", "")),
            source=str(data.get("source", "")),
            revision=_coerce_int(data.get("revision")),
            etag=str(data.get("etag", "")),
            updated_at_unix_ms=_coerce_int(data.get("updated_at_unix_ms")),
            document=document if isinstance(document, dict) else {},
            validation=validation if isinstance(validation, dict) else {},
            can_undo=bool(can_undo) if can_undo is not None else None,
            can_redo=bool(can_redo) if can_redo is not None else None,
        )


@dataclass(frozen=True)
class MobpackDraftSummary:
    """A draft as draft results list it: its id, revision and etag and what
    the Flow Editor library shows of it, without its document, archive or
    validation. ``meta/mobpacks/get`` returns a draft's document.
    """
    id: str
    name: str
    version: str
    stage: str
    trigger: str
    source: str
    description: str
    revision: int
    etag: str
    updated_at_unix_ms: int
    can_undo: bool
    can_redo: bool

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackDraftSummary:
        return cls(
            id=str(data.get("id", "")),
            name=str(data.get("name", "")),
            version=str(data.get("version", "")),
            stage=str(data.get("stage", "")),
            trigger=str(data.get("trigger", "")),
            source=str(data.get("source", "")),
            description=str(data.get("description", "")),
            revision=_coerce_int(data.get("revision")),
            etag=str(data.get("etag", "")),
            updated_at_unix_ms=_coerce_int(data.get("updated_at_unix_ms")),
            can_undo=bool(data.get("can_undo", False)),
            can_redo=bool(data.get("can_redo", False)),
        )


@dataclass(frozen=True)
class MobpackDraftListResult:
    """Result of a meta/mobpacks/list RPC call."""
    source: str
    store_path: str | None
    runtime_backed: bool
    rows: list[MobpackDraftSummary]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackDraftListResult:
        return cls(
            source=str(data.get("source", "")),
            store_path=data.get("store_path"),
            runtime_backed=bool(data.get("runtime_backed", False)),
            rows=[MobpackDraftSummary.from_dict(r) for r in data.get("rows", [])],
        )


@dataclass(frozen=True)
class MobpackDraftGetResult:
    """Result of a meta/mobpacks/get RPC call."""
    source: str
    store_path: str | None
    runtime_backed: bool
    row: MobpackDraftRow

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackDraftGetResult:
        return cls(
            source=str(data.get("source", "")),
            store_path=data.get("store_path"),
            runtime_backed=bool(data.get("runtime_backed", False)),
            row=MobpackDraftRow.from_dict(data.get("row", {})),
        )


@dataclass(frozen=True)
class MobpackDraftSaveResult:
    """Result of a meta/mobpacks/create or meta/mobpacks/save RPC call."""
    source: str
    store_path: str | None
    row: MobpackDraftRow
    rows: list[MobpackDraftSummary]
    durability_warning: str | None = None
    """Set when Meta wrote the draft but could not sync the draft store's
    directory: the write took effect and may not survive a crash."""

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackDraftSaveResult:
        return cls(
            source=str(data.get("source", "")),
            store_path=data.get("store_path"),
            row=MobpackDraftRow.from_dict(data.get("row", {})),
            rows=[MobpackDraftSummary.from_dict(r) for r in data.get("rows", [])],
            durability_warning=data.get("durability_warning"),
        )


@dataclass(frozen=True)
class MobpackDraftHistoryResult:
    """Result of a meta/mobpacks/undo or meta/mobpacks/redo RPC call.

    ``stepped`` is False (with a ``reason``) when there is no history or
    future entry to step to; the draft is left untouched in that case.
    """
    source: str
    store_path: str | None
    stepped: bool
    row: MobpackDraftRow
    rows: list[MobpackDraftSummary]
    reason: str | None = None
    durability_warning: str | None = None
    """Set when Meta stepped the draft but could not sync the draft store's
    directory: the step took effect and may not survive a crash."""

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackDraftHistoryResult:
        return cls(
            source=str(data.get("source", "")),
            store_path=data.get("store_path"),
            stepped=bool(data.get("stepped", False)),
            row=MobpackDraftRow.from_dict(data.get("row", {})),
            rows=[MobpackDraftSummary.from_dict(r) for r in data.get("rows", [])],
            reason=data.get("reason"),
            durability_warning=data.get("durability_warning"),
        )


@dataclass(frozen=True)
class MobpackDraftDeleteResult:
    """Result of a meta/mobpacks/delete RPC call."""
    source: str
    store_path: str | None
    id: str
    deleted: bool
    rows: list[MobpackDraftSummary]
    durability_warning: str | None = None
    """Set when Meta deleted the draft but could not sync the draft store's
    directory: the delete took effect and may not survive a crash."""

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackDraftDeleteResult:
        return cls(
            source=str(data.get("source", "")),
            store_path=data.get("store_path"),
            id=str(data.get("id", "")),
            deleted=bool(data.get("deleted", False)),
            rows=[MobpackDraftSummary.from_dict(r) for r in data.get("rows", [])],
            durability_warning=data.get("durability_warning"),
        )


@dataclass(frozen=True)
class MobpackApplyOperationResult:
    """Result of a meta/mobpacks/apply_operation RPC call."""
    source: str
    operation: str
    ok: bool
    document: dict[str, Any]
    selection: dict[str, Any] | None
    validation: MobpackValidationResult

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackApplyOperationResult:
        selection = data.get("selection")
        return cls(
            source=str(data.get("source", "")),
            operation=str(data.get("operation", "")),
            ok=bool(data.get("ok", False)),
            document=dict(data.get("document", {})),
            selection=selection if isinstance(selection, dict) else None,
            validation=MobpackValidationResult.from_dict(data.get("validation", {})),
        )


@dataclass(frozen=True)
class MobpackDeployCommandResult:
    """Result of a meta/mobpacks/deploy_command RPC call."""
    command: str
    argv: list[str]
    deploy_command: str
    filename: str
    validation: MobpackValidationResult
    source: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackDeployCommandResult:
        return cls(
            command=str(data.get("command", "")),
            argv=list(data.get("argv", [])),
            deploy_command=str(data.get("deploy_command", "")),
            filename=str(data.get("filename", "")),
            validation=MobpackValidationResult.from_dict(data.get("validation", {})),
            source=str(data.get("source", "")),
        )


@dataclass(frozen=True)
class MobpackDeployResult:
    """Result of a meta/mobpacks/deploy RPC call."""
    filename: str
    pack_path: str
    pack_sha256: str
    command: str
    argv: list[str]
    plan_trace: list[dict[str, Any]]
    executed: bool
    success: bool
    validation: MobpackValidationResult
    display_rows: list[MobpackDisplayRow]
    status_code: int | None = None
    stdout: str | None = None
    stderr: str | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobpackDeployResult:
        return cls(
            filename=str(data.get("filename", "")),
            pack_path=str(data.get("pack_path", "")),
            pack_sha256=str(data.get("pack_sha256", "")),
            command=str(data.get("command", "")),
            argv=list(data.get("argv", [])),
            plan_trace=list(data.get("plan_trace", [])),
            executed=bool(data.get("executed", False)),
            success=bool(data.get("success", False)),
            validation=MobpackValidationResult.from_dict(data.get("validation", {})),
            display_rows=[
                MobpackDisplayRow.from_dict(r) for r in data.get("display_rows", [])
            ],
            status_code=data.get("status_code"),
            stdout=data.get("stdout"),
            stderr=data.get("stderr"),
        )


class MobMemberStatus(str, Enum):
    """Execution status for a mob member."""
    ACTIVE = "active"
    RETIRING = "retiring"
    BROKEN = "broken"
    COMPLETED = "completed"
    UNKNOWN = "unknown"


@dataclass(frozen=True)
class MobUnreachablePeer:
    """A wired peer known to be unreachable."""
    peer: str
    reason: str | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobUnreachablePeer:
        return cls(
            peer=data.get("peer", ""),
            reason=data.get("reason"),
        )


@dataclass(frozen=True)
class PeerConnectivitySnapshot:
    """Live connectivity for a member's wired peers.

    meerkat 0.7.x projects this as a tri-state, internally-tagged object:
    ``{"status": "known", "snapshot": {...}}`` carries the counts, while
    ``{"status": "not_applicable"}`` (no bridge session backs the member) and
    ``{"status": "probe_timed_out"}`` (the live probe did not resolve in time)
    carry no counts. ``status`` distinguishes the three; the counts are only
    meaningful when ``status == "known"``. The legacy flat shape (counts at the
    top level, no ``status``) is still accepted for backward compatibility.
    """
    reachable_peer_count: int
    unknown_peer_count: int
    unreachable_peers: list[MobUnreachablePeer]
    status: str = "known"

    @property
    def is_known(self) -> bool:
        """True when the snapshot carries resolved peer counts."""
        return self.status == "known"

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> PeerConnectivitySnapshot:
        # 0.7.x tri-state: read counts from `.snapshot` behind the `status`
        # discriminator. `not_applicable` / `probe_timed_out` carry no snapshot.
        status = str(data.get("status", "known"))
        snapshot = data.get("snapshot")
        if isinstance(snapshot, dict):
            counts = snapshot
        else:
            # Legacy flat shape (no `status`/`snapshot`): counts at top level.
            counts = data
        return cls(
            reachable_peer_count=int(counts.get("reachable_peer_count", 0)),
            unknown_peer_count=int(counts.get("unknown_peer_count", 0)),
            unreachable_peers=[
                MobUnreachablePeer.from_dict(p)
                for p in counts.get("unreachable_peers", [])
            ],
            status=status,
        )


@dataclass(frozen=True)
class MemberProgressSnapshot:
    """Machine-owned live execution/progress projection (meerkat 0.7.29).

    ``run_state`` is ``idle``/``run_open``/``unknown``; ``health`` is
    ``healthy``/``degraded``/``wedged``/``unknown``; ``last_progress_event``
    is ``execution_advanced``/``became_idle``/``unchanged``. All three are
    open vocabularies — tolerate future values.
    """
    run_state: str
    in_flight_work: int
    last_progress_at_ms: int
    last_progress_event: str
    health: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MemberProgressSnapshot:
        return cls(
            run_state=str(data.get("run_state", "unknown")),
            in_flight_work=int(data.get("in_flight_work", 0)),
            last_progress_at_ms=int(data.get("last_progress_at_ms", 0)),
            last_progress_event=str(data.get("last_progress_event", "unchanged")),
            health=str(data.get("health", "unknown")),
        )


@dataclass(frozen=True)
class RichMemberSnapshot:
    """Rich execution snapshot from meta/member_status."""
    status: str
    output_preview: str | None
    error: str | None
    tokens_used: int
    is_final: bool
    current_session_id: str | None
    peer_connectivity: PeerConnectivitySnapshot | None = None
    progress: MemberProgressSnapshot | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> RichMemberSnapshot:
        pc_raw = data.get("peer_connectivity")
        progress_raw = data.get("progress")
        return cls(
            status=data.get("status", "unknown"),
            output_preview=data.get("output_preview"),
            error=data.get("error"),
            tokens_used=int(data.get("tokens_used", 0)),
            is_final=bool(data.get("is_final", False)),
            current_session_id=data.get("current_session_id"),
            peer_connectivity=PeerConnectivitySnapshot.from_dict(pc_raw) if pc_raw else None,
            progress=MemberProgressSnapshot.from_dict(progress_raw) if progress_raw else None,
        )


@dataclass(frozen=True)
class HelperResult:
    """Result from spawn_helper or fork_helper."""
    output: str | None
    tokens_used: int
    session_id: str | None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> HelperResult:
        return cls(
            output=data.get("output"),
            tokens_used=int(data.get("tokens_used", 0)),
            session_id=data.get("session_id"),
        )



@dataclass(frozen=True)
class MobRunSnapshot:
    """Flow run snapshot from meta/flow_status."""
    run_id: str
    mob_id: str
    flow_id: str
    status: str
    step_ledger: list[dict[str, Any]]
    failure_ledger: list[dict[str, Any]]
    # The members each role of a role-targeted run dispatches to, as the
    # runtime recorded them; ``None`` for a role-wide run.
    role_targets: dict[str, list[str]] | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> MobRunSnapshot:
        return cls(
            run_id=data.get("run_id", ""),
            mob_id=data.get("mob_id", ""),
            flow_id=data.get("flow_id", ""),
            status=data.get("status", "unknown"),
            step_ledger=list(data.get("step_ledger", [])),
            failure_ledger=list(data.get("failure_ledger", [])),
            role_targets=_role_targets_from(data.get("role_targets")),
        )


class ErrorCategory(str, Enum):
    """Error event categories matching Rust's ErrorEvent variants.

    Every Rust ``ErrorEvent`` variant must appear here with its serde
    ``snake_case`` wire tag. The Rust test
    ``crates/meerkat-meta/tests/sdk_error_category_parity.rs`` fails the build if
    this set drifts from the Rust enum.
    """
    SPAWN_FAILURE = "spawn_failure"
    RECONCILE_INCOMPLETE = "reconcile_incomplete"
    CHECKPOINT_FAILURE = "checkpoint_failure"
    COMPACTION_PERSISTENCE_REJECTED = "compaction_persistence_rejected"
    ACTOR_LOOP_STALLED = "actor_loop_stalled"
    ACTOR_LOOP_RECOVERED = "actor_loop_recovered"
    ACTOR_LOOP_TERMINATED = "actor_loop_terminated"
    HOST_LOOP_CRASH = "host_loop_crash"
    REDISCOVER_FAILURE = "rediscover_failure"
    EVENT_LOG_FLUSH_FAILURE = "event_log_flush_failure"
    IDENTITY_MATERIALIZATION_FAILURE = "identity_materialization_failure"
    MOB_STOP_PROCEEDED_WITHOUT_INTERRUPT = "mob_stop_proceeded_without_interrupt"
    RETIRED_MEMBER_RELEASE_FAILED = "retired_member_release_failed"
    MOB_ANNOUNCEMENT_ESCALATED = "mob_announcement_escalated"
    MOB_ANNOUNCEMENT_ESCALATION_RESOLVED = "mob_announcement_escalation_resolved"


#: Categories that report a failure ENDING rather than starting. Mirrors the
#: Rust runtime's ``log_error_event``, which logs these at INFO instead of
#: ERROR. A receiver that pages on every ``on_error`` call would otherwise
#: raise a second incident for a stall that already resolved.
#:
#: Holds wire strings, not enum members: ``ErrorEvent.category`` carries the
#: raw wire value, so membership must not depend on ``Enum`` hash semantics.
RESOLUTION_ERROR_CATEGORIES = frozenset(
    {
        ErrorCategory.ACTOR_LOOP_RECOVERED.value,
        ErrorCategory.MOB_ANNOUNCEMENT_ESCALATION_RESOLVED.value,
    }
)


def _escalation_subject(subject: Any) -> str:
    """A mob announcement escalation's subject as the runtime words it."""
    if not isinstance(subject, dict):
        return str(subject or "")
    kind = subject.get("kind")
    if kind == "source":
        return f"source {subject.get('source_id', '')} of mob {subject.get('mob_id', '')}"
    if kind == "member":
        member = subject.get("member") if isinstance(subject.get("member"), dict) else {}
        return f"member {member.get('identity', '')} of mob {member.get('mob_id', '')}"
    if kind == "declaration":
        return f"declared {subject.get('record_kind', '')} {subject.get('id', '')}"
    return json.dumps(subject, sort_keys=True)


@dataclass(frozen=True)
class ErrorEvent:
    """Operational error event for alerting.

    Matches Rust's ``ErrorEvent`` enum. The ``category`` field corresponds
    to the enum variant, and ``context`` carries the variant's fields.

    Usage::

        async def on_error(event: ErrorEvent):
            if event.category == ErrorCategory.SPAWN_FAILURE:
                member_id = event.context["member_id"]
                await alerts.send(f"spawn failed: {member_id}: {event.message}")
    """
    category: str
    message: str
    context: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ErrorEvent:
        category = data.get("category", "unknown")
        context = {k: v for k, v in data.items() if k != "category"}
        # Build a human-readable message from the context
        error = context.get("error", "")
        member_id = context.get("member_id", "")
        if category == ErrorCategory.SPAWN_FAILURE:
            message = f"{member_id}: {error}" if member_id else error
        elif category == ErrorCategory.RECONCILE_INCOMPLETE:
            failures = context.get("failures", 0)
            skipped = context.get("skipped", 0)
            message = f"{failures} failures, {skipped} skipped"
        elif category == ErrorCategory.CHECKPOINT_FAILURE:
            session_id = context.get("session_id", "")
            message = f"{session_id}: {error}" if session_id else error
        elif category == ErrorCategory.COMPACTION_PERSISTENCE_REJECTED:
            identity = context.get("identity", "")
            session_id = context.get("session_id", "")
            message = f"{identity} ({session_id}): {error}"
        elif category == ErrorCategory.ACTOR_LOOP_STALLED:
            waited = context.get("probe_waited_secs", 0)
            detail = context.get("detail", "")
            message = f"probe unanswered for {waited}s: {detail}"
        elif category == ErrorCategory.ACTOR_LOOP_RECOVERED:
            stall_id = context.get("stall_id", 0)
            stalled_for = context.get("stalled_for_secs", 0)
            message = f"stall {stall_id} resolved after {stalled_for}s"
        elif category == ErrorCategory.ACTOR_LOOP_TERMINATED:
            stall_id = context.get("stall_id")
            detail = context.get("detail", "")
            observed = f" (observed on stall {stall_id})" if stall_id is not None else ""
            message = (
                f"the mob actor loop is gone{observed}; this is NOT a recovery, "
                f"the process must restart: {detail}"
            )
        elif category == ErrorCategory.HOST_LOOP_CRASH:
            message = f"{member_id}: {error}" if member_id else error
        elif category == ErrorCategory.REDISCOVER_FAILURE:
            message = error
        elif category == ErrorCategory.EVENT_LOG_FLUSH_FAILURE:
            message = error
        elif category == ErrorCategory.IDENTITY_MATERIALIZATION_FAILURE:
            identity = context.get("identity", "")
            initiator = context.get("initiator", "")
            operation = context.get("operation", "")
            target = f"{identity} for {initiator}" if initiator else identity
            message = ": ".join(str(part) for part in (target, operation, error) if part)
        elif category == ErrorCategory.MOB_STOP_PROCEEDED_WITHOUT_INTERRUPT:
            waited_ms = context.get("waited_ms", 0)
            member = context.get("member") or ""
            subject = f" on {member}" if member else ""
            message = (
                f"teardown proceeded after {waited_ms}ms WITHOUT a successful "
                f"interrupt{subject}; a turn may have been admitted and may "
                f"still be running: {error}"
            )
        elif category == ErrorCategory.RETIRED_MEMBER_RELEASE_FAILED:
            identity = context.get("identity", "")
            runtime_id = context.get("agent_runtime_id", "")
            cause = context.get("cause", "")
            message = (
                f"{identity} ({runtime_id}) was retired ({cause}), and the host's "
                f"release failed: {error}"
            )
        elif category == ErrorCategory.MOB_ANNOUNCEMENT_ESCALATED:
            escalation = context.get("escalation")
            escalation = escalation if isinstance(escalation, dict) else {}
            message = (
                f"{context.get('event', '')} {escalation.get('escalation_id', '')} "
                f"{escalation.get('cause', '')} on "
                f"{_escalation_subject(escalation.get('subject'))} (notified again "
                f"{context.get('renotify_count', 0)} times, cursor {context.get('cursor', 0)})"
            )
        elif category == ErrorCategory.MOB_ANNOUNCEMENT_ESCALATION_RESOLVED:
            message = (
                f"{context.get('escalation_id', '')} {context.get('cause', '')} on "
                f"{_escalation_subject(context.get('subject'))} (cursor {context.get('cursor', 0)})"
            )
        else:
            message = str(data)
        return cls(category=category, message=message, context=context)

    @property
    def is_resolution(self) -> bool:
        """True when this reports a failure ENDING rather than starting.

        Lets a host close an incident instead of opening a second one. See
        :data:`RESOLUTION_ERROR_CATEGORIES`.
        """
        return self.category in RESOLUTION_ERROR_CATEGORIES


# -----------------------------------------------------------------
# WorkGraph — collaborative work-item graph
# -----------------------------------------------------------------
#
# Wire fields are a verbatim serde of meerkat-workgraph's `WorkItem`,
# `WorkEdge`, `WorkAttentionBinding`, `WorkGraphSnapshot`, etc. Nested
# substructures with no dedicated wrapper type on this SDK surface
# (`completion_policy`, `owner`, `claim`, `machine_state`, `work_ref`,
# `target`, `status`, `projection_policy`, `payload`, ...) pass through as
# raw dicts/lists rather than being re-modeled here.


@dataclass(frozen=True)
class WorkGraphItem:
    """A WorkGraph work item (verbatim serde of upstream `WorkItem`)."""
    id: str
    realm_id: str
    namespace: str
    title: str
    description: str | None
    status: str
    completion_policy: dict[str, Any]
    priority: str
    labels: list[str]
    owner: dict[str, Any] | None
    claim: dict[str, Any] | None
    machine_state: dict[str, Any]
    revision: int
    due_at: str | None
    not_before: str | None
    snoozed_until: str | None
    created_at: str
    updated_at: str
    terminal_at: str | None
    external_refs: list[Any]
    evidence_refs: list[Any]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> WorkGraphItem:
        return cls(
            id=str(data.get("id", "")),
            realm_id=str(data.get("realm_id", "")),
            namespace=str(data.get("namespace", "")),
            title=str(data.get("title", "")),
            description=data.get("description"),
            status=str(data.get("status", "")),
            completion_policy=dict(data.get("completion_policy") or {}),
            priority=str(data.get("priority", "")),
            labels=list(data.get("labels", [])),
            owner=data.get("owner"),
            claim=data.get("claim"),
            machine_state=dict(data.get("machine_state") or {}),
            revision=_coerce_int(data.get("revision")),
            due_at=data.get("due_at"),
            not_before=data.get("not_before"),
            snoozed_until=data.get("snoozed_until"),
            created_at=str(data.get("created_at", "")),
            updated_at=str(data.get("updated_at", "")),
            terminal_at=data.get("terminal_at"),
            external_refs=list(data.get("external_refs", [])),
            evidence_refs=list(data.get("evidence_refs", [])),
        )


@dataclass(frozen=True)
class WorkGraphEdge:
    """A directed edge between two WorkGraph items (verbatim `WorkEdge`)."""
    realm_id: str
    namespace: str
    kind: str
    from_id: str
    to_id: str
    created_at: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> WorkGraphEdge:
        return cls(
            realm_id=str(data.get("realm_id", "")),
            namespace=str(data.get("namespace", "")),
            kind=str(data.get("kind", "")),
            from_id=str(data.get("from_id", "")),
            to_id=str(data.get("to_id", "")),
            created_at=str(data.get("created_at", "")),
        )


@dataclass(frozen=True)
class WorkGraphAttentionBinding:
    """An attention binding wiring a work item to a goal target (verbatim
    `WorkAttentionBinding`)."""
    binding_id: str
    work_ref: dict[str, Any]
    target: dict[str, Any]
    mode: str
    status: dict[str, Any]
    machine_state: dict[str, Any]
    delegated_authority: str | None
    projection_policy: dict[str, Any]
    created_at: str
    updated_at: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> WorkGraphAttentionBinding:
        return cls(
            binding_id=str(data.get("binding_id", "")),
            work_ref=dict(data.get("work_ref") or {}),
            target=dict(data.get("target") or {}),
            mode=str(data.get("mode", "")),
            status=dict(data.get("status") or {}),
            machine_state=dict(data.get("machine_state") or {}),
            delegated_authority=data.get("delegated_authority"),
            projection_policy=dict(data.get("projection_policy") or {}),
            created_at=str(data.get("created_at", "")),
            updated_at=str(data.get("updated_at", "")),
        )


@dataclass(frozen=True)
class WorkGraphSnapshotResult:
    """Result of `workgraph_snapshot` — the richest WorkGraph payload."""
    realm_id: str
    namespace: str | None
    all_namespaces: bool
    captured_at: str
    event_high_water_mark: int | None
    items: list[WorkGraphItem]
    edges: list[WorkGraphEdge]
    attention: list[WorkGraphAttentionBinding]
    ready_item_ids: list[str]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> WorkGraphSnapshotResult:
        hwm = data.get("event_high_water_mark")
        return cls(
            realm_id=str(data.get("realm_id", "")),
            namespace=data.get("namespace"),
            all_namespaces=bool(data.get("all_namespaces", False)),
            captured_at=str(data.get("captured_at", "")),
            event_high_water_mark=_coerce_int(hwm) if hwm is not None else None,
            items=[WorkGraphItem.from_dict(i) for i in data.get("items", [])],
            edges=[WorkGraphEdge.from_dict(e) for e in data.get("edges", [])],
            attention=[
                WorkGraphAttentionBinding.from_dict(a) for a in data.get("attention", [])
            ],
            ready_item_ids=list(data.get("ready_item_ids", [])),
        )


@dataclass(frozen=True)
class WorkGraphItemsResult:
    """Result envelope for `workgraph_list` / `workgraph_ready`."""
    items: list[WorkGraphItem]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> WorkGraphItemsResult:
        raw_items = data.get("items", []) if isinstance(data, dict) else []
        return cls(items=[WorkGraphItem.from_dict(i) for i in raw_items])


@dataclass(frozen=True)
class WorkGraphGoalResult:
    """Result of a goal mutation: the goal work item + its attention binding."""
    item: WorkGraphItem
    attention: WorkGraphAttentionBinding

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> WorkGraphGoalResult:
        return cls(
            item=WorkGraphItem.from_dict(data.get("item") or {}),
            attention=WorkGraphAttentionBinding.from_dict(data.get("attention") or {}),
        )


@dataclass(frozen=True)
class WorkGraphAttentionReassignResult:
    """Result of `workgraph_attention_reassign`: the superseded and new
    bindings."""
    previous: WorkGraphAttentionBinding
    attention: WorkGraphAttentionBinding

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> WorkGraphAttentionReassignResult:
        return cls(
            previous=WorkGraphAttentionBinding.from_dict(data.get("previous") or {}),
            attention=WorkGraphAttentionBinding.from_dict(data.get("attention") or {}),
        )


@dataclass(frozen=True)
class WorkGraphEventEntry:
    """A single entry from the WorkGraph event log (verbatim `WorkGraphEvent`)."""
    seq: int | None
    realm_id: str
    namespace: str
    item_id: str | None
    kind: str
    at: str
    payload: Any

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> WorkGraphEventEntry:
        seq = data.get("seq")
        return cls(
            seq=_coerce_int(seq) if seq is not None else None,
            realm_id=str(data.get("realm_id", "")),
            namespace=str(data.get("namespace", "")),
            item_id=data.get("item_id"),
            kind=str(data.get("kind", "")),
            at=str(data.get("at", "")),
            payload=data.get("payload"),
        )


_RUN_STOP_OUTCOMES = ("stopped", "not_current", "not_stoppable")


@dataclass(frozen=True)
class MemberRunStopContributor:
    """One input that contributed to a stopped member run.

    ``completion`` is the completion class meerkat delivered (a cancelled
    joined steer reports ``runtime_terminated``); ``terminal`` is the
    committed input terminal class (``cancelled`` for a stopped run's inputs),
    ``None`` only when no store retains the row.
    """

    input_id: str
    completion: str
    terminal: str | None


@dataclass(frozen=True)
class MemberRunStopReceipt:
    """meerkat's typed run-stop receipt, relayed by ``meta/stop_member_run``.

    ``outcome`` is ``stopped`` (``contributors`` lists every input bound to the
    run), ``not_current`` (the run is no longer current; nothing was touched,
    ``current_run_id`` names the live run if any), or ``not_stoppable`` (a
    runtime stop or teardown owns the run; ``state`` is its runtime state).
    """

    outcome: str
    run_id: str
    contributors: tuple[MemberRunStopContributor, ...] = ()
    current_run_id: str | None = None
    state: str | None = None

    @classmethod
    def from_dict(cls, data: Any) -> MemberRunStopReceipt:
        if not isinstance(data, dict):
            raise ValueError(f"run-stop receipt must be an object: {data!r}")
        outcome = data.get("outcome")
        run_id = data.get("run_id")
        if outcome not in _RUN_STOP_OUTCOMES or not isinstance(run_id, str):
            raise ValueError(f"invalid run-stop receipt: {data!r}")
        contributors: list[MemberRunStopContributor] = []
        state = data.get("state")
        current = data.get("current_run_id")
        if outcome == "stopped":
            rows = data.get("contributors")
            if not isinstance(rows, list):
                raise ValueError(f"stopped receipt needs contributors: {data!r}")
            for row in rows:
                if (
                    not isinstance(row, dict)
                    or not isinstance(row.get("input_id"), str)
                    or not isinstance(row.get("completion"), str)
                ):
                    raise ValueError(f"malformed run-stop contributor: {row!r}")
                terminal = row.get("terminal")
                contributors.append(
                    MemberRunStopContributor(
                        input_id=row["input_id"],
                        completion=row["completion"],
                        terminal=terminal if isinstance(terminal, str) else None,
                    )
                )
        elif outcome == "not_stoppable" and not isinstance(state, str):
            raise ValueError(f"not_stoppable receipt needs a state: {data!r}")
        elif outcome == "not_current" and current is not None and not isinstance(current, str):
            raise ValueError(f"current_run_id must be a string: {data!r}")
        return cls(
            outcome=outcome,
            run_id=run_id,
            contributors=tuple(contributors),
            current_run_id=current if isinstance(current, str) else None,
            state=state if isinstance(state, str) else None,
        )


# -- Mob announcements ------------------------------------------------------
#
# Result models for ``meta/mob_announcements/*``. Parsing is forward-tolerant:
# unknown fields are ignored and every wire vocabulary (states, phases,
# statuses, outcomes, actions, reasons, kinds) passes through as a string, so
# a newer gateway's value reaches the caller instead of failing the parse.
# Nested declarations the caller wrote (a source's identity principal,
# audience, start, delivery, health policy, ...) are kept as the gateway sent
# them.

MOB_ANNOUNCEMENT_SOURCES_V1 = "mob_announcements.sources.v1"


def _wire_dict(value: Any) -> dict[str, Any]:
    return dict(value) if isinstance(value, dict) else {}


def _wire_dict_or_none(value: Any) -> dict[str, Any] | None:
    return dict(value) if isinstance(value, dict) else None


def _wire_dicts(value: Any) -> tuple[dict[str, Any], ...]:
    return tuple(dict(item) for item in value if isinstance(item, dict)) if isinstance(value, list) else ()


def _wire_strs(value: Any) -> tuple[str, ...]:
    return tuple(str(item) for item in value) if isinstance(value, list) else ()


def _wire_int_or_none(value: Any) -> int | None:
    return value if isinstance(value, int) and not isinstance(value, bool) else None


def _wire_str_or_none(value: Any) -> str | None:
    return value if isinstance(value, str) else None


def _wire_kind(value: Any, key: str = "kind") -> str:
    return str(value.get(key, "")) if isinstance(value, dict) else str(value or "")


@dataclass(frozen=True)
class AnnouncementMember:
    """The member a subscription delivers to: an identity within a mob."""

    mob_id: str
    identity: str

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementMember:
        d = _wire_dict(data)
        return cls(mob_id=str(d.get("mob_id", "")), identity=str(d.get("identity", "")))

    def to_dict(self) -> dict[str, str]:
        return {"mob_id": self.mob_id, "identity": self.identity}


@dataclass(frozen=True)
class AnnouncementRef:
    """One announcement by its stable identity: its mob and source key."""

    mob_id: str
    source_key: str

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementRef:
        d = _wire_dict(data)
        return cls(mob_id=str(d.get("mob_id", "")), source_key=str(d.get("source_key", "")))


@dataclass(frozen=True)
class Announcer:
    """Who made an announcement.

    ``kind`` is ``"host"``, ``"member"`` (with ``identity``), ``"source"``
    (with ``source_id``) or a newer kind; ``raw`` keeps the object as sent.
    """

    kind: str
    identity: str | None = None
    source_id: str | None = None
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: Any) -> Announcer:
        d = _wire_dict(data)
        return cls(
            kind=str(d.get("kind", "")),
            identity=_wire_str_or_none(d.get("identity")),
            source_id=_wire_str_or_none(d.get("source_id")),
            raw=d,
        )


@dataclass(frozen=True)
class Announcement:
    """A committed, immutable mob announcement."""

    mob_id: str
    sequence: int
    announced_by: Announcer
    source_key: str
    topics: tuple[str, ...]
    body: str
    digest: str
    announced_at_ms: int
    correlation_id: str | None = None
    causes: tuple[AnnouncementRef, ...] = ()

    @classmethod
    def from_dict(cls, data: Any) -> Announcement:
        d = _wire_dict(data)
        causes = d.get("causes")
        return cls(
            mob_id=str(d.get("mob_id", "")),
            sequence=_coerce_int(d.get("sequence")),
            announced_by=Announcer.from_dict(d.get("announced_by")),
            source_key=str(d.get("source_key", "")),
            topics=_wire_strs(d.get("topics")),
            body=str(d.get("body", "")),
            digest=str(d.get("digest", "")),
            announced_at_ms=_coerce_int(d.get("announced_at_ms")),
            correlation_id=_wire_str_or_none(d.get("correlation_id")),
            causes=tuple(AnnouncementRef.from_dict(cause) for cause in causes)
            if isinstance(causes, list)
            else (),
        )


@dataclass(frozen=True)
class AnnounceResult:
    """The answer to an announce: the committed announcement, whether it
    replayed an identical earlier one, and how many subscriptions captured
    it (a count, never their names)."""

    announcement: Announcement
    replayed: bool
    captured: int

    @classmethod
    def from_dict(cls, data: Any) -> AnnounceResult:
        d = _wire_dict(data)
        return cls(
            announcement=Announcement.from_dict(d.get("announcement")),
            replayed=bool(d.get("replayed", False)),
            captured=_coerce_int(d.get("captured")),
        )


@dataclass(frozen=True)
class AnnouncementSubscription:
    """A durable subscription.

    ``state`` is ``"active"``, ``"paused"``, ``"cancelled"``, ``"ended"`` or a
    newer state. ``frontier`` maps each mob to the sequence after which the
    selector in force captures; ``ended`` says why an ended subscription
    ended. ``delivery`` is how its member takes what it captures, as set on
    the subscription (``{"await_handling"}``), and ``await_handling`` reads
    it; both are ``None`` when it was not set, which leaves the gateway's
    default.
    """

    id: str
    member: AnnouncementMember
    topics: tuple[str, ...]
    mobs: tuple[str, ...]
    state: str
    revision: int
    selector_revision: int
    frontier: dict[str, int]
    created_at_ms: int
    updated_at_ms: int
    ended_mobs: tuple[str, ...] = ()
    ended: dict[str, Any] | None = None
    member_mob_cursor: int | None = None
    delivery: dict[str, Any] | None = None
    await_handling: bool | None = None

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementSubscription:
        d = _wire_dict(data)
        selector = _wire_dict(d.get("selector"))
        frontier = _wire_dict(d.get("frontier"))
        delivery = _wire_dict_or_none(d.get("delivery"))
        await_handling = delivery.get("await_handling") if delivery else None
        return cls(
            id=str(d.get("id", "")),
            member=AnnouncementMember.from_dict(d.get("member")),
            topics=_wire_strs(selector.get("topics")),
            mobs=_wire_strs(selector.get("mobs")),
            state=str(d.get("state", "")),
            revision=_coerce_int(d.get("revision")),
            selector_revision=_coerce_int(d.get("selector_revision")),
            frontier={str(mob): _coerce_int(sequence) for mob, sequence in frontier.items()},
            created_at_ms=_coerce_int(d.get("created_at_ms")),
            updated_at_ms=_coerce_int(d.get("updated_at_ms")),
            ended_mobs=_wire_strs(d.get("ended_mobs")),
            ended=_wire_dict_or_none(d.get("ended")),
            member_mob_cursor=_wire_int_or_none(d.get("member_mob_cursor")),
            delivery=delivery,
            await_handling=await_handling if isinstance(await_handling, bool) else None,
        )


@dataclass(frozen=True)
class AnnouncementRoleSelector:
    """The members a role template takes: every member of ``mob_id`` its
    roster records with ``role`` (the profile it was spawned from)."""

    mob_id: str
    role: str

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementRoleSelector:
        d = _wire_dict(data)
        return cls(mob_id=str(d.get("mob_id", "")), role=str(d.get("role", "")))

    def to_dict(self) -> dict[str, str]:
        return {"mob_id": self.mob_id, "role": self.role}


@dataclass(frozen=True)
class AnnouncementRoleTemplate:
    """A role template: a subscription for every member of a mob with a
    role. It never delivers by itself: each member it takes captures through
    a child subscription with the id ``"<template id>/<16 hex digits>"``.

    ``state`` is ``"active"``, ``"paused"``, ``"cancelled"`` or a newer
    state. ``delivery`` is every child's delivery, as set on the template
    (``{"await_handling"}``), and ``await_handling`` reads it; both are
    ``None`` when it was not set.
    """

    id: str
    members: AnnouncementRoleSelector
    topics: tuple[str, ...]
    mobs: tuple[str, ...]
    state: str
    revision: int
    created_at_ms: int
    updated_at_ms: int
    delivery: dict[str, Any] | None = None
    await_handling: bool | None = None

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementRoleTemplate:
        d = _wire_dict(data)
        delivery = _wire_dict_or_none(d.get("delivery"))
        await_handling = delivery.get("await_handling") if delivery else None
        return cls(
            id=str(d.get("id", "")),
            members=AnnouncementRoleSelector.from_dict(d.get("members")),
            topics=_wire_strs(d.get("topics")),
            mobs=_wire_strs(d.get("mobs")),
            state=str(d.get("state", "")),
            revision=_coerce_int(d.get("revision")),
            created_at_ms=_coerce_int(d.get("created_at_ms")),
            updated_at_ms=_coerce_int(d.get("updated_at_ms")),
            delivery=delivery,
            await_handling=await_handling if isinstance(await_handling, bool) else None,
        )


@dataclass(frozen=True)
class AnnouncementSubscriptionChange:
    """A subscription pause, resume or cancel: the subscription as it is
    now, and the batches that still hold its announcements and have not
    settled (a change never rewrites a sealed batch).

    The same verbs on a role template's id change the template and its
    children in one step: then ``template`` and ``children`` are set and
    ``subscription`` is ``None``.
    """

    subscription: AnnouncementSubscription | None
    unresolved_batches: tuple[str, ...]
    template: AnnouncementRoleTemplate | None = None
    children: tuple[AnnouncementSubscription, ...] = ()

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementSubscriptionChange:
        d = _wire_dict(data)
        template = d.get("template")
        if isinstance(template, dict):
            children = d.get("children")
            return cls(
                subscription=None,
                unresolved_batches=_wire_strs(d.get("unresolved_batches")),
                template=AnnouncementRoleTemplate.from_dict(template),
                children=tuple(AnnouncementSubscription.from_dict(child) for child in children)
                if isinstance(children, list)
                else (),
            )
        return cls(
            subscription=AnnouncementSubscription.from_dict(d.get("subscription")),
            unresolved_batches=_wire_strs(d.get("unresolved_batches")),
        )


@dataclass(frozen=True)
class AnnouncementSuspendMarker:
    """Why a member's deliveries wait now, and on what evidence.

    ``cause_kind`` is the cause's ``kind`` (``"peer_composing"``,
    ``"member_not_live"``, ... or a newer one); ``cause`` keeps the object as
    sent.
    """

    cause: dict[str, Any]
    cause_kind: str
    evidence: str
    since_ms: int

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementSuspendMarker:
        d = _wire_dict(data)
        cause = _wire_dict(d.get("cause"))
        return cls(
            cause=cause,
            cause_kind=str(cause.get("kind", "")),
            evidence=str(d.get("evidence", "")),
            since_ms=_coerce_int(d.get("since_ms")),
        )


@dataclass(frozen=True)
class AnnouncementSubscriptionView:
    """``subscriptions/get``: the subscription (``None`` when unknown), why
    its member's deliveries wait if they do, and its backlog when the
    gateway reports one. A role template's id reads the template as
    ``template``, beside no subscription."""

    subscription: AnnouncementSubscription | None
    marker: AnnouncementSuspendMarker | None = None
    backlog: dict[str, Any] | None = None
    template: AnnouncementRoleTemplate | None = None

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementSubscriptionView:
        d = _wire_dict(data)
        subscription = d.get("subscription")
        marker = d.get("marker")
        template = d.get("template")
        return cls(
            subscription=AnnouncementSubscription.from_dict(subscription)
            if isinstance(subscription, dict)
            else None,
            marker=AnnouncementSuspendMarker.from_dict(marker) if isinstance(marker, dict) else None,
            backlog=_wire_dict_or_none(d.get("backlog")),
            template=AnnouncementRoleTemplate.from_dict(template) if isinstance(template, dict) else None,
        )


@dataclass(frozen=True)
class AnnouncementBatchItem:
    """One captured announcement in a batch, under the subscription revision
    that captured it."""

    subscription_id: str
    subscription_revision: int
    mob_id: str
    sequence: int
    source_key: str

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementBatchItem:
        d = _wire_dict(data)
        return cls(
            subscription_id=str(d.get("subscription_id", "")),
            subscription_revision=_coerce_int(d.get("subscription_revision")),
            mob_id=str(d.get("mob_id", "")),
            sequence=_coerce_int(d.get("sequence")),
            source_key=str(d.get("source_key", "")),
        )


@dataclass(frozen=True)
class AnnouncementBatchHandling:
    """What became of a delivered batch's turn, as Meta read it, and what it
    did about it.

    ``state`` is ``"pending"``, ``"handled"``, ``"handled_unverified"``,
    ``"unhandled"``, ``"unknown"`` or newer. ``cause`` is why the turn did not
    complete (the receipt's completion type, or the abandon reason when there
    is none). ``redelivered`` counts the items owed again with a notice,
    ``held`` the items held for an operator, and ``dismissed`` the held items
    an operator settled.
    """

    state: str
    read_at_ms: int
    cause: str | None = None
    redelivered: int = 0
    held: int = 0
    dismissed: int = 0

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementBatchHandling:
        d = _wire_dict(data)
        return cls(
            state=str(d.get("state", "")),
            read_at_ms=_coerce_int(d.get("read_at_ms")),
            cause=_wire_str_or_none(d.get("cause")),
            redelivered=_coerce_int(d.get("redelivered")),
            held=_coerce_int(d.get("held")),
            dismissed=_coerce_int(d.get("dismissed")),
        )


@dataclass(frozen=True)
class AnnouncementBatch:
    """An immutable delivery batch for one member.

    ``state`` is the delivery state's tag (``"sealed"``, ``"submitting"``,
    ``"committed"``, ``"delivered"``, ``"refused"``, ...; a newer tag passes
    through) and ``state_detail`` the full state object. ``gaps`` are the
    announcements owed to the member that this batch reports as not
    delivered (``{"mob_id", "count", "reason"}``). ``route`` is ``"local"``,
    ``"child"``, ``"peer"`` or newer. ``handling`` is ``None`` until the batch
    is admitted, and from a gateway that does not report it.
    """

    id: str
    member: AnnouncementMember
    items: tuple[AnnouncementBatchItem, ...]
    route: str
    body: str
    content_digest: str
    state: str
    state_detail: dict[str, Any]
    sealed_at_ms: int
    updated_at_ms: int
    gaps: tuple[dict[str, Any], ...] = ()
    predecessor: str | None = None
    successor: str | None = None
    handling: AnnouncementBatchHandling | None = None

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementBatch:
        d = _wire_dict(data)
        state = _wire_dict(d.get("state"))
        items = d.get("items")
        handling = d.get("handling")
        return cls(
            id=str(d.get("id", "")),
            member=AnnouncementMember.from_dict(d.get("member")),
            items=tuple(AnnouncementBatchItem.from_dict(item) for item in items)
            if isinstance(items, list)
            else (),
            route=_wire_kind(d.get("route")),
            body=str(d.get("body", "")),
            content_digest=str(d.get("content_digest", "")),
            state=str(state.get("state", "")),
            state_detail=state,
            sealed_at_ms=_coerce_int(d.get("sealed_at_ms")),
            updated_at_ms=_coerce_int(d.get("updated_at_ms")),
            gaps=_wire_dicts(d.get("gaps")),
            predecessor=_wire_str_or_none(d.get("predecessor")),
            successor=_wire_str_or_none(d.get("successor")),
            handling=AnnouncementBatchHandling.from_dict(handling) if isinstance(handling, dict) else None,
        )


@dataclass(frozen=True)
class AnnouncementDeliveryObservation:
    """``deliveries/observe`` and ``deliveries/reconcile``: the batch, the
    native owner's live status (``native``; ``native_status`` is its
    ``status``), the admitted input's own state once admitted, and why the
    member's deliveries wait, if they do."""

    batch: AnnouncementBatch
    native: dict[str, Any]
    native_status: str
    input: dict[str, Any] | None = None
    marker: AnnouncementSuspendMarker | None = None
    handling: AnnouncementBatchHandling | None = None

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementDeliveryObservation:
        d = _wire_dict(data)
        native = _wire_dict(d.get("native"))
        marker = d.get("marker")
        handling = d.get("handling")
        return cls(
            batch=AnnouncementBatch.from_dict(d.get("batch")),
            native=native,
            native_status=str(native.get("status", "")),
            input=_wire_dict_or_none(d.get("input")),
            marker=AnnouncementSuspendMarker.from_dict(marker) if isinstance(marker, dict) else None,
            handling=AnnouncementBatchHandling.from_dict(handling) if isinstance(handling, dict) else None,
        )


@dataclass(frozen=True)
class AnnouncementSourceIdentity:
    """A source's identity: its provider, the principal it collects as
    (``{"kind", ...}``; a list shows only ``{"kind", "digest"}``) and its
    scope (``{"kind", "id"}``)."""

    provider: str
    principal: dict[str, Any]
    scope: dict[str, Any]

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementSourceIdentity:
        d = _wire_dict(data)
        return cls(
            provider=str(d.get("provider", "")),
            principal=_wire_dict(d.get("principal")),
            scope=_wire_dict(d.get("scope")),
        )


@dataclass(frozen=True)
class AnnouncementSourceRoute:
    """One of a source's routes: ``kind`` is ``"direct"``, ``"triage"`` or a
    newer kind; ``raw`` keeps the route as sent."""

    kind: str
    raw: dict[str, Any]

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementSourceRoute:
        d = _wire_dict(data)
        return cls(kind=str(d.get("kind", "")), raw=d)


@dataclass(frozen=True)
class AnnouncementSourceCursor:
    """A source's progress: the version every collector step compares and
    sets, and the adapter's opaque token (``sources/get`` only; a list
    leaves it out)."""

    version: int
    token: str | None = None
    advanced_at_ms: int | None = None

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementSourceCursor:
        d = _wire_dict(data)
        return cls(
            version=_coerce_int(d.get("version")),
            token=_wire_str_or_none(d.get("token")),
            advanced_at_ms=_wire_int_or_none(d.get("advanced_at_ms")),
        )


@dataclass(frozen=True)
class AnnouncementSourceRunner:
    """The host runnable that collects a source, and how often it runs."""

    host_runnable: str
    every_seconds: int

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementSourceRunner:
        d = _wire_dict(data)
        return cls(host_runnable=str(d.get("host_runnable", "")), every_seconds=_coerce_int(d.get("every_seconds")))


@dataclass(frozen=True)
class AnnouncementSourceHealth:
    """A source's health.

    ``status`` is ``"baselining"``, ``"backfilling"``, ``"ok"``,
    ``"failing"``, ``"stale"``, ``"paused"``, ``"ended"`` or newer.
    ``totals`` counts what its collections did (``announced``, ``replayed``,
    ``conflicts``, ``own_writes``, ``refusals``, ``gaps``). ``escalations``
    are its open escalations. ``runner_scheduled_here`` says whether this
    gateway schedules its runner (``None`` without a runner, or from a gateway
    that does not report it); a runner on a gateway that schedules none never
    fires there. ``quiet_monitoring`` and ``empty_coverage_monitoring`` say
    whether quiet and empty coverage detection are configured.
    """

    status: str
    consecutive_failures: int
    totals: dict[str, int]
    last_success_at_ms: int | None = None
    last_new_item_at_ms: int | None = None
    last_observed: dict[str, Any] | None = None
    last_observed_at_ms: int | None = None
    empty_snapshot_since_ms: int | None = None
    last_failure: dict[str, Any] | None = None
    last_conflict: dict[str, Any] | None = None
    escalations: tuple[AnnouncementEscalation, ...] = ()
    quiet_monitoring: dict[str, Any] | None = None
    empty_coverage_monitoring: dict[str, Any] | None = None
    runner_scheduled_here: bool | None = None

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementSourceHealth:
        d = _wire_dict(data)
        totals = _wire_dict(d.get("totals"))
        escalations = d.get("escalations")
        scheduled = d.get("runner_scheduled_here")
        return cls(
            status=str(d.get("status", "")),
            consecutive_failures=_coerce_int(d.get("consecutive_failures")),
            totals={str(name): _coerce_int(count) for name, count in totals.items()},
            last_success_at_ms=_wire_int_or_none(d.get("last_success_at_ms")),
            last_new_item_at_ms=_wire_int_or_none(d.get("last_new_item_at_ms")),
            last_observed=_wire_dict_or_none(d.get("last_observed")),
            last_observed_at_ms=_wire_int_or_none(d.get("last_observed_at_ms")),
            empty_snapshot_since_ms=_wire_int_or_none(d.get("empty_snapshot_since_ms")),
            last_failure=_wire_dict_or_none(d.get("last_failure")),
            last_conflict=_wire_dict_or_none(d.get("last_conflict")),
            escalations=tuple(AnnouncementEscalation.from_dict(item) for item in escalations)
            if isinstance(escalations, list)
            else (),
            quiet_monitoring=_wire_dict_or_none(d.get("quiet_monitoring")),
            empty_coverage_monitoring=_wire_dict_or_none(d.get("empty_coverage_monitoring")),
            runner_scheduled_here=scheduled if isinstance(scheduled, bool) else None,
        )


@dataclass(frozen=True)
class AnnouncementSource:
    """A durable announcement source, one generation of it.

    ``state`` is ``"active"``, ``"paused"``, ``"ended"`` or newer;
    ``collection`` is ``"baselining"``, ``"backfilling"``, ``"live"`` or
    newer. ``revision`` rises on every change, across generations, and every
    collector step names it with the cursor version. ``streams`` counts the
    live and onboarding streams.
    """

    mob_id: str
    source_id: str
    generation: int
    revision: int
    identity: AnnouncementSourceIdentity
    observation: dict[str, Any]
    topics: tuple[str, ...]
    audience: tuple[dict[str, Any], ...]
    routes: tuple[AnnouncementSourceRoute, ...]
    state: str
    collection: str
    cursor: AnnouncementSourceCursor
    health: AnnouncementSourceHealth
    created_at_ms: int
    updated_at_ms: int
    connection: dict[str, Any] | None = None
    conversation_members: tuple[AnnouncementMember, ...] = ()
    start: dict[str, Any] = field(default_factory=dict)
    delivery: dict[str, Any] = field(default_factory=dict)
    health_policy: dict[str, Any] = field(default_factory=dict)
    runner: AnnouncementSourceRunner | None = None
    streams: dict[str, int] = field(default_factory=dict)
    backfilled: int = 0

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementSource:
        d = _wire_dict(data)
        routes = d.get("routes")
        members = d.get("conversation_members")
        runner = d.get("runner")
        streams = _wire_dict(d.get("streams"))
        return cls(
            mob_id=str(d.get("mob_id", "")),
            source_id=str(d.get("source_id", "")),
            generation=_coerce_int(d.get("generation")),
            revision=_coerce_int(d.get("revision")),
            identity=AnnouncementSourceIdentity.from_dict(d.get("identity")),
            observation=_wire_dict(d.get("observation")),
            topics=_wire_strs(d.get("topics")),
            audience=_wire_dicts(d.get("audience")),
            routes=tuple(AnnouncementSourceRoute.from_dict(route) for route in routes)
            if isinstance(routes, list)
            else (),
            state=str(d.get("state", "")),
            collection=str(d.get("collection", "")),
            cursor=AnnouncementSourceCursor.from_dict(d.get("cursor")),
            health=AnnouncementSourceHealth.from_dict(d.get("health")),
            created_at_ms=_coerce_int(d.get("created_at_ms")),
            updated_at_ms=_coerce_int(d.get("updated_at_ms")),
            connection=_wire_dict_or_none(d.get("connection")),
            conversation_members=tuple(AnnouncementMember.from_dict(member) for member in members)
            if isinstance(members, list)
            else (),
            start=_wire_dict(d.get("start")),
            delivery=_wire_dict(d.get("delivery")),
            health_policy=_wire_dict(d.get("health_policy")),
            runner=AnnouncementSourceRunner.from_dict(runner) if isinstance(runner, dict) else None,
            streams={str(name): _coerce_int(count) for name, count in streams.items()},
            backfilled=_coerce_int(d.get("backfilled")),
        )


@dataclass(frozen=True)
class AnnouncementSourcePutResult:
    """``sources/put``: the source as declared. ``affected_subscriptions``
    and ``delivery_readiness`` are ``None`` from a gateway that does not
    report them."""

    source: AnnouncementSource
    affected_subscriptions: int | None = None
    delivery_readiness: dict[str, Any] | None = None

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementSourcePutResult:
        d = _wire_dict(data)
        return cls(
            source=AnnouncementSource.from_dict(d.get("source")),
            affected_subscriptions=_wire_int_or_none(d.get("affected_subscriptions")),
            delivery_readiness=_wire_dict_or_none(d.get("delivery_readiness")),
        )


@dataclass(frozen=True)
class AnnouncementSourceStream:
    """A stream of a source as ``sources/get`` lists it: its phase, its
    onboarding mode, the history its backfill announced, and when it
    entered its phase."""

    stream: str
    phase: str
    backfilled: int = 0
    mode: dict[str, Any] | str | None = None
    max_items: int | None = None
    since_ms: int | None = None

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementSourceStream:
        d = _wire_dict(data)
        mode = d.get("mode")
        return cls(
            stream=str(d.get("stream", "")),
            phase=str(d.get("phase", "")),
            backfilled=_coerce_int(d.get("backfilled")),
            mode=dict(mode) if isinstance(mode, dict) else mode if isinstance(mode, str) else None,
            max_items=_wire_int_or_none(d.get("max_items")),
            since_ms=_wire_int_or_none(d.get("since_ms")),
        )


@dataclass(frozen=True)
class AnnouncementSourceView:
    """``sources/get``: the source (``None`` when this mob has none by that
    id), and its streams when asked for them."""

    source: AnnouncementSource | None
    streams: tuple[AnnouncementSourceStream, ...] | None = None

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementSourceView:
        d = _wire_dict(data)
        source = d.get("source")
        streams = d.get("streams")
        return cls(
            source=AnnouncementSource.from_dict(source) if isinstance(source, dict) else None,
            streams=tuple(AnnouncementSourceStream.from_dict(stream) for stream in streams)
            if isinstance(streams, list)
            else None,
        )


@dataclass(frozen=True)
class SourceItemOutcome:
    """What became of one item of a collector step.

    ``outcome`` is ``"announced"``, ``"replayed"``, ``"conflict"`` or newer.
    ``sequence`` is the announcement's (for a conflict, the original's);
    ``captured`` counts the subscriptions that captured it.
    """

    item_id: str
    version: str
    source_key: str
    outcome: str
    sequence: int | None = None
    captured: int | None = None
    existing_digest: str | None = None
    offered_digest: str | None = None

    @classmethod
    def from_dict(cls, data: Any) -> SourceItemOutcome:
        d = _wire_dict(data)
        return cls(
            item_id=str(d.get("item_id", "")),
            version=str(d.get("version", "")),
            source_key=str(d.get("source_key", "")),
            outcome=str(d.get("outcome", "")),
            sequence=_wire_int_or_none(d.get("sequence")),
            captured=_wire_int_or_none(d.get("captured")),
            existing_digest=_wire_str_or_none(d.get("existing_digest")),
            offered_digest=_wire_str_or_none(d.get("offered_digest")),
        )


@dataclass(frozen=True)
class SourceStreamPhase:
    """A stream's phase after a step that onboarded it."""

    stream: str
    phase: str

    @classmethod
    def from_dict(cls, data: Any) -> SourceStreamPhase:
        d = _wire_dict(data)
        return cls(stream=str(d.get("stream", "")), phase=str(d.get("phase", "")))


@dataclass(frozen=True)
class SourceAdvanceResult:
    """``sources/advance``: the source as the step committed it, each item's
    outcome, and the phases of the streams it onboarded. ``replayed`` says
    the step was answered from its receipt."""

    source: AnnouncementSource
    replayed: bool
    items: tuple[SourceItemOutcome, ...]
    streams: tuple[SourceStreamPhase, ...]

    @classmethod
    def from_dict(cls, data: Any) -> SourceAdvanceResult:
        d = _wire_dict(data)
        items = d.get("items")
        streams = d.get("streams")
        return cls(
            source=AnnouncementSource.from_dict(d.get("source")),
            replayed=bool(d.get("replayed", False)),
            items=tuple(SourceItemOutcome.from_dict(item) for item in items) if isinstance(items, list) else (),
            streams=tuple(SourceStreamPhase.from_dict(stream) for stream in streams)
            if isinstance(streams, list)
            else (),
        )


@dataclass(frozen=True)
class AnnouncementAuditEntry:
    """One counted audit entry: every refusal or withholding of one member
    by one generation of one source, for one reason, and every hold,
    redelivery or dismissal of an item whose turn ended unhandled.

    ``action`` is ``"subscription_refused"``, ``"capture_withheld"``,
    ``"items_held"``, ``"items_released"``, ``"expired_while_held"``,
    ``"delivered_across_revocation"``, ``"redelivered"``, ``"dismissed"`` or
    newer; ``reason`` is ``"audience_not_covered"``, ``"no_direct_route"``,
    ``"route_overlap"``, ``"route_conflict"``, ``"unhandled"`` or newer.
    ``source_id`` and ``generation`` are ``None`` for a plain announcement.
    """

    id: int
    at_ms: int
    action: str
    mob_id: str
    source_id: str | None
    generation: int | None
    member: AnnouncementMember
    reason: str
    count: int
    first_at_ms: int
    last_at_ms: int

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementAuditEntry:
        d = _wire_dict(data)
        return cls(
            id=_coerce_int(d.get("id")),
            at_ms=_coerce_int(d.get("at_ms")),
            action=str(d.get("action", "")),
            mob_id=str(d.get("mob_id", "")),
            source_id=_wire_str_or_none(d.get("source_id")),
            generation=_wire_int_or_none(d.get("generation")),
            member=AnnouncementMember.from_dict(d.get("member")),
            reason=str(d.get("reason", "")),
            count=_coerce_int(d.get("count")),
            first_at_ms=_coerce_int(d.get("first_at_ms")),
            last_at_ms=_coerce_int(d.get("last_at_ms")),
        )


def _escalation_subject_kind(subject: dict[str, Any]) -> str:
    return str(subject.get("kind", ""))


@dataclass(frozen=True)
class AnnouncementEscalation:
    """A durable mob announcement escalation.

    ``cause`` is ``"failure_streak"``, ``"stale"``, ``"quiet"``,
    ``"empty_coverage"``, ``"source_gap"``, ``"conflicts"``, ``"suspended"``,
    ``"member_gap"``, ``"audience_hold"``, ``"unhandled"``,
    ``"handled_unverified"``, ``"declaration_pending"`` or newer.
    ``subject`` is ``{"kind": "source", "mob_id", "source_id"}``,
    ``{"kind": "member", "member"}``, ``{"kind": "declaration", "record_kind",
    "id"}`` or newer, as sent; ``subject_kind`` is its kind.
    ``resolved_at_ms`` is ``None`` while it is open.
    """

    escalation_id: str
    cause: str
    subject: dict[str, Any]
    subject_kind: str
    since_ms: int
    raised_at_ms: int
    renotify_count: int
    resolved_at_ms: int | None = None
    evidence: str | None = None

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementEscalation:
        d = _wire_dict(data)
        subject = _wire_dict(d.get("subject"))
        return cls(
            escalation_id=str(d.get("escalation_id", "")),
            cause=str(d.get("cause", "")),
            subject=subject,
            subject_kind=_escalation_subject_kind(subject),
            since_ms=_coerce_int(d.get("since_ms")),
            raised_at_ms=_coerce_int(d.get("raised_at_ms")),
            renotify_count=_coerce_int(d.get("renotify_count")),
            resolved_at_ms=_wire_int_or_none(d.get("resolved_at_ms")),
            evidence=_wire_str_or_none(d.get("evidence")),
        )


@dataclass(frozen=True)
class AnnouncementEscalationEvent:
    """One raise, notification or resolution of an escalation, at its event
    cursor. ``event`` is ``"raised"``, ``"renotified"``, ``"resolved"`` or
    newer."""

    cursor: int
    escalation_id: str
    event: str
    cause: str
    subject: dict[str, Any]
    subject_kind: str
    since_ms: int
    at_ms: int
    renotify_count: int

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementEscalationEvent:
        d = _wire_dict(data)
        subject = _wire_dict(d.get("subject"))
        return cls(
            cursor=_coerce_int(d.get("cursor")),
            escalation_id=str(d.get("escalation_id", "")),
            event=str(d.get("event", "")),
            cause=str(d.get("cause", "")),
            subject=subject,
            subject_kind=_escalation_subject_kind(subject),
            since_ms=_coerce_int(d.get("since_ms")),
            at_ms=_coerce_int(d.get("at_ms")),
            renotify_count=_coerce_int(d.get("renotify_count")),
        )


@dataclass(frozen=True)
class AnnouncementEscalationsPage:
    """``escalations/list``: the events after a cursor (``events``), or with
    ``open=True`` the open escalations (``escalations``); the other is
    ``None``. ``next`` is the cursor to read on from."""

    next: int
    events: tuple[AnnouncementEscalationEvent, ...] | None = None
    escalations: tuple[AnnouncementEscalation, ...] | None = None

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementEscalationsPage:
        d = _wire_dict(data)
        events = d.get("events")
        escalations = d.get("escalations")
        return cls(
            next=_coerce_int(d.get("next")),
            events=tuple(AnnouncementEscalationEvent.from_dict(item) for item in events)
            if isinstance(events, list)
            else None,
            escalations=tuple(AnnouncementEscalation.from_dict(item) for item in escalations)
            if isinstance(escalations, list)
            else None,
        )


@dataclass(frozen=True)
class AnnouncementMemberBacklog:
    """What a member is owed: deliverable items, held and withheld counts."""

    eligible_count: int
    eligible_bytes: int
    outstanding: bool
    held_audience: int
    held_unhandled: int
    held_refused: int
    withheld_audience: int
    oldest_captured_at_ms: int | None = None
    last_sealed_at_ms: int | None = None

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementMemberBacklog:
        d = _wire_dict(data)
        return cls(
            eligible_count=_coerce_int(d.get("eligible_count")),
            eligible_bytes=_coerce_int(d.get("eligible_bytes")),
            outstanding=bool(d.get("outstanding", False)),
            held_audience=_coerce_int(d.get("held_audience")),
            held_unhandled=_coerce_int(d.get("held_unhandled")),
            held_refused=_coerce_int(d.get("held_refused")),
            withheld_audience=_coerce_int(d.get("withheld_audience")),
            oldest_captured_at_ms=_wire_int_or_none(d.get("oldest_captured_at_ms")),
            last_sealed_at_ms=_wire_int_or_none(d.get("last_sealed_at_ms")),
        )


@dataclass(frozen=True)
class AnnouncementMemberHandling:
    """What became of a member's admitted batches' turns: batches by handling
    (``handled_unverified`` is never folded into ``handled``), the items it
    holds for an operator now (``held``), and when its oldest unhandled turn
    was read."""

    pending: int
    handled: int
    handled_unverified: int
    unhandled: int
    held: int
    oldest_unhandled_at_ms: int | None = None

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementMemberHandling:
        d = _wire_dict(data)
        return cls(
            pending=_coerce_int(d.get("pending")),
            handled=_coerce_int(d.get("handled")),
            handled_unverified=_coerce_int(d.get("handled_unverified")),
            unhandled=_coerce_int(d.get("unhandled")),
            held=_coerce_int(d.get("held")),
            oldest_unhandled_at_ms=_wire_int_or_none(d.get("oldest_unhandled_at_ms")),
        )


@dataclass(frozen=True)
class AnnouncementMemberView:
    """``members/get`` and each ``members/list`` entry: a member's backlog,
    why its deliveries wait (``marker``, persisted), its open escalations,
    and what became of its turns (``handling``, ``None`` from a gateway that
    does not report it)."""

    member: AnnouncementMember
    backlog: AnnouncementMemberBacklog
    marker: AnnouncementSuspendMarker | None = None
    escalations: tuple[AnnouncementEscalation, ...] = ()
    handling: AnnouncementMemberHandling | None = None

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementMemberView:
        d = _wire_dict(data)
        marker = d.get("marker")
        escalations = d.get("escalations")
        handling = d.get("handling")
        return cls(
            member=AnnouncementMember.from_dict(d.get("member")),
            backlog=AnnouncementMemberBacklog.from_dict(d.get("backlog")),
            marker=AnnouncementSuspendMarker.from_dict(marker) if isinstance(marker, dict) else None,
            escalations=tuple(AnnouncementEscalation.from_dict(item) for item in escalations)
            if isinstance(escalations, list)
            else (),
            handling=AnnouncementMemberHandling.from_dict(handling) if isinstance(handling, dict) else None,
        )


@dataclass(frozen=True)
class AnnouncementDeclarationRecord:
    """One record the applied declarations manage.

    ``kind`` is ``"source"``, ``"subscription"``, ``"role_child"`` or newer;
    ``status`` is ``"applied"``, ``"pending"``, ``"left"`` (a role child whose
    member left the role), ``"suppressed"`` (ended or cancelled by an
    operator) or newer. ``refusal`` is ``{"kind", "message"}``, the refusal a
    pending record's put met, and ``refusal_kind`` its kind. An applied
    source carries ``collection`` (``"baselining"``, ``"backfilling"``,
    ``"live"``, ``"failing"``, ``"stale"`` or newer); an applied subscription
    or role child carries ``delivery`` (``{"kind": "ready"}`` or ``{"kind":
    "refused", "cause"}``), and ``delivery_kind`` its kind.
    """

    kind: str
    id: str
    status: str
    refusal: dict[str, Any] | None = None
    refusal_kind: str | None = None
    collection: str | None = None
    delivery: dict[str, Any] | None = None
    delivery_kind: str | None = None

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementDeclarationRecord:
        d = _wire_dict(data)
        refusal = _wire_dict_or_none(d.get("refusal"))
        delivery = _wire_dict_or_none(d.get("delivery"))
        return cls(
            kind=str(d.get("kind", "")),
            id=str(d.get("id", "")),
            status=str(d.get("status", "")),
            refusal=refusal,
            refusal_kind=_wire_str_or_none(refusal.get("kind")) if refusal else None,
            collection=_wire_str_or_none(d.get("collection")),
            delivery=delivery,
            delivery_kind=_wire_str_or_none(delivery.get("kind")) if delivery else None,
        )


@dataclass(frozen=True)
class AnnouncementDeclarationsView:
    """``declarations/get`` and ``declarations/apply``: the applied
    ``revision`` (0 before any), its ``digest``, and one record per declared
    source and subscription and per child of a declared role template."""

    revision: int
    digest: str
    records: tuple[AnnouncementDeclarationRecord, ...]

    @classmethod
    def from_dict(cls, data: Any) -> AnnouncementDeclarationsView:
        d = _wire_dict(data)
        records = d.get("records")
        return cls(
            revision=_coerce_int(d.get("revision")),
            digest=str(d.get("digest", "")),
            records=tuple(AnnouncementDeclarationRecord.from_dict(record) for record in records)
            if isinstance(records, list)
            else (),
        )


@dataclass(frozen=True)
class MobAnnouncementsStatus:
    """``meta/status.mob_announcements``: sources by state (``active``,
    ``paused``, ``failing``, ``stale``), members (``suspended``,
    ``unhandled_held``), admitted batches by handling (``pending``,
    ``handled``, ``handled_unverified``, ``unhandled``; empty from a gateway
    that does not report them), and the open escalations with the last event
    cursor."""

    sources: dict[str, int]
    members: dict[str, int]
    open_escalations: int
    last_event_cursor: int | None = None
    handling: dict[str, int] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: Any) -> MobAnnouncementsStatus:
        d = _wire_dict(data)
        escalations = _wire_dict(d.get("escalations"))
        return cls(
            sources={str(key): _coerce_int(value) for key, value in _wire_dict(d.get("sources")).items()},
            members={str(key): _coerce_int(value) for key, value in _wire_dict(d.get("members")).items()},
            open_escalations=_coerce_int(escalations.get("open")),
            last_event_cursor=_wire_int_or_none(escalations.get("last_event_cursor")),
            handling={str(key): _coerce_int(value) for key, value in _wire_dict(d.get("handling")).items()},
        )


@dataclass(frozen=True)
class MobAnnouncementsCapability:
    """``meta/capabilities.mob_announcements``: what this gateway's mob
    announcements offer.

    ``sources`` is ``"v1"`` when sources are served. ``local_delivery`` is
    ``{"kind": "ready"}``, ``{"kind": "refused", "cause": ...}``,
    ``{"kind": "unavailable", "detail": ...}`` or newer;
    ``local_delivery_kind`` is its kind. ``governed_admission`` says whether
    deliveries to governed runtimes are admitted.
    """

    sources: str | None = None
    local_delivery: dict[str, Any] | None = None
    local_delivery_kind: str | None = None
    governed_admission: bool = False

    @classmethod
    def from_dict(cls, data: Any) -> MobAnnouncementsCapability:
        d = _wire_dict(data)
        local_delivery = _wire_dict_or_none(d.get("local_delivery"))
        return cls(
            sources=_wire_str_or_none(d.get("sources")),
            local_delivery=local_delivery,
            local_delivery_kind=_wire_str_or_none(local_delivery.get("kind")) if local_delivery else None,
            governed_admission=bool(d.get("governed_admission", False)),
        )


def supports_announcement_sources(capabilities: CapabilitiesResult | list[str] | tuple[str, ...]) -> bool:
    """Whether the gateway serves mob announcement sources: its
    ``feature_capabilities`` name ``"mob_announcements.sources.v1"``.

    Takes a :class:`CapabilitiesResult` or its ``feature_capabilities``. A
    gateway that lists ``meta/mob_announcements/announce`` among its methods
    without this capability runs mob announcements without sources; an older
    one answers the source methods with method-not-found.
    """
    features = (
        capabilities.feature_capabilities
        if isinstance(capabilities, CapabilitiesResult)
        else capabilities
    )
    return MOB_ANNOUNCEMENT_SOURCES_V1 in features


FLOW_ROLE_TARGETS = "flow_role_targets"


def supports_flow_role_targets(capabilities: CapabilitiesResult | list[str] | tuple[str, ...]) -> bool:
    """Whether ``meta/run_flow`` accepts ``role_targets`` on this gateway: its
    ``feature_capabilities`` name ``"flow_role_targets"``.

    Takes a :class:`CapabilitiesResult` or its ``feature_capabilities``. A
    gateway without it ignores the parameter and runs the flow role-wide, so
    a caller checks this before sending ``role_targets``
    (:meth:`MobHandle.run_flow` does).
    """
    features = (
        capabilities.feature_capabilities
        if isinstance(capabilities, CapabilitiesResult)
        else capabilities
    )
    return FLOW_ROLE_TARGETS in features


FLOW_RUN_IDEMPOTENCY_KEY = "flow_run_idempotency_key"


def supports_flow_run_idempotency_key(capabilities: CapabilitiesResult | list[str] | tuple[str, ...]) -> bool:
    """Whether ``meta/run_flow`` accepts ``idempotency_key`` on this gateway:
    its ``feature_capabilities`` name ``"flow_run_idempotency_key"``.

    Takes a :class:`CapabilitiesResult` or its ``feature_capabilities``. A
    gateway without it ignores the key, so a retry would admit a second run:
    a caller checks this before sending a key (:meth:`MobHandle.run_flow`
    does).
    """
    features = (
        capabilities.feature_capabilities
        if isinstance(capabilities, CapabilitiesResult)
        else capabilities
    )
    return FLOW_RUN_IDEMPOTENCY_KEY in features
