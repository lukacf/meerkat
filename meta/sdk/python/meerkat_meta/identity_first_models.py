"""Identity-first models for Meta SDK (REQ-40, REQ-43, REQ-43a)."""
from __future__ import annotations

import copy

from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Literal

from .errors import ContractMismatchError

from .models import ToolArtifactRef

_VALID_ORIGINS = frozenset({"connector", "scheduler", "policy", "flow", "system"})
MAX_IDENTITY_BACKGROUND_WARM_CONCURRENCY = 16

IdentityBootstrapModeName = Literal[
    "eager_materialize",
    "lazy_materialize",
    "lazy_with_background_warm",
]


def _required_bool(data: dict[str, Any], field_name: str) -> bool:
    if field_name not in data:
        raise ValueError(f"identity bootstrap status requires {field_name!r}")
    value = data[field_name]
    if type(value) is not bool:
        raise TypeError(f"identity bootstrap {field_name} must be a boolean")
    return value


def _optional_bool(
    data: dict[str, Any],
    field_name: str,
    *,
    default: bool | None,
) -> bool | None:
    if field_name not in data:
        return default
    value = data[field_name]
    if type(value) is not bool:
        raise TypeError(f"identity bootstrap {field_name} must be a boolean")
    return value


def _nonnegative_int(data: dict[str, Any], field_name: str) -> int:
    value = data.get(field_name, 0)
    if type(value) is not int:
        raise TypeError(
            f"identity bootstrap count {field_name!r} must be an integer"
        )
    if value < 0:
        raise ValueError(
            f"identity bootstrap count {field_name!r} must be non-negative"
        )
    return value


@dataclass(frozen=True)
class IdentityBootstrapMode:
    """Identity materialization policy used while the gateway starts."""

    mode: IdentityBootstrapModeName
    concurrency: int | None = None

    def __post_init__(self) -> None:
        supported = {
            "eager_materialize",
            "lazy_materialize",
            "lazy_with_background_warm",
        }
        if self.mode not in supported:
            raise ValueError(f"unsupported identity bootstrap mode: {self.mode!r}")
        if self.mode == "lazy_with_background_warm":
            if (
                not isinstance(self.concurrency, int)
                or isinstance(self.concurrency, bool)
                or self.concurrency <= 0
            ):
                raise ValueError(
                    "lazy_with_background_warm requires a positive integer concurrency"
                )
            if self.concurrency > MAX_IDENTITY_BACKGROUND_WARM_CONCURRENCY:
                raise ValueError(
                    "lazy_with_background_warm concurrency must be at most "
                    f"{MAX_IDENTITY_BACKGROUND_WARM_CONCURRENCY}"
                )
        elif self.concurrency is not None:
            raise ValueError(f"{self.mode} does not accept concurrency")

    @classmethod
    def eager_materialize(cls) -> IdentityBootstrapMode:
        return cls(mode="eager_materialize")

    @classmethod
    def lazy_materialize(cls) -> IdentityBootstrapMode:
        return cls(mode="lazy_materialize")

    @classmethod
    def lazy_with_background_warm(
        cls, *, concurrency: int
    ) -> IdentityBootstrapMode:
        return cls(mode="lazy_with_background_warm", concurrency=concurrency)

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {"mode": self.mode}
        if self.concurrency is not None:
            result["concurrency"] = self.concurrency
        return result

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> IdentityBootstrapMode:
        if not isinstance(data, dict):
            raise TypeError("identity bootstrap mode must be an object")
        mode = data.get("mode")
        if not isinstance(mode, str):
            raise ValueError("identity bootstrap mode requires a string mode")
        unsupported = sorted(set(data) - {"mode", "concurrency"})
        if unsupported:
            raise ValueError(
                "identity bootstrap mode has unsupported field(s): "
                + ", ".join(unsupported)
            )
        return cls(mode=mode, concurrency=data.get("concurrency"))  # type: ignore[arg-type]


class IdentityBootstrapState(str, Enum):
    """Transient materialization state for one identity during bootstrap."""

    DORMANT = "dormant"
    WARMING = "warming"
    ACTIVE = "active"
    BROKEN = "broken"
    UNKNOWN = "unknown"

    @classmethod
    def parse(cls, raw: Any) -> IdentityBootstrapState:
        if isinstance(raw, cls):
            return raw
        if isinstance(raw, str):
            try:
                return cls(raw)
            except ValueError:
                pass
        return cls.UNKNOWN


@dataclass(frozen=True)
class IdentityBootstrapCounts:
    """Aggregate bootstrap-state counts. ``deferred`` counts the identities a
    candidate launch left to the authoritative launch; it is ``0`` when the
    gateway omits it."""

    dormant: int = 0
    warming: int = 0
    active: int = 0
    broken: int = 0
    deferred: int = 0

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> IdentityBootstrapCounts:
        if not isinstance(data, dict):
            raise TypeError("identity bootstrap counts must be an object")
        return cls(
            dormant=_nonnegative_int(data, "dormant"),
            warming=_nonnegative_int(data, "warming"),
            active=_nonnegative_int(data, "active"),
            broken=_nonnegative_int(data, "broken"),
            deferred=_nonnegative_int(data, "deferred"),
        )

    def to_dict(self) -> dict[str, int]:
        result = {
            "dormant": self.dormant,
            "warming": self.warming,
            "active": self.active,
            "broken": self.broken,
        }
        if self.deferred:
            result["deferred"] = self.deferred
        return result


@dataclass(frozen=True)
class IdentityRestoreProgress:
    """Typed eager-restore progress for one identity, published on the
    bootstrap status while the restore pass runs.

    ``stage`` is ``registered``, ``resuming``, ``minted``, ``resumed`` or
    ``refused`` (callers must tolerate future values). A ``refused`` stage
    carries the refusal's JSON-RPC ``code`` (the code the same refusal
    answers a call with: ``-32051``, ``-32052``, ``-32053`` or ``-32603``)
    and its ``kind`` when the refusal has one.
    """

    stage: str
    kind: str | None = None
    code: int | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> IdentityRestoreProgress:
        if not isinstance(data, dict) or not isinstance(data.get("stage"), str):
            raise TypeError("restore progress must be an object with a string stage")
        kind = data.get("kind")
        code = data.get("code")
        return cls(
            stage=data["stage"],
            kind=kind if isinstance(kind, str) else None,
            code=code if isinstance(code, int) else None,
        )

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {"stage": self.stage}
        if self.code is not None:
            result["code"] = self.code
        if self.kind is not None:
            result["kind"] = self.kind
        return result


@dataclass(frozen=True)
class IdentityBootstrapEntry:
    identity: str
    state: IdentityBootstrapState
    error: str | None = None
    restore: IdentityRestoreProgress | None = None

    @classmethod
    def from_dict(
        cls, identity: str, data: dict[str, Any]
    ) -> IdentityBootstrapEntry:
        if not isinstance(data, dict):
            raise TypeError(f"bootstrap status for {identity!r} must be an object")
        if "state" not in data:
            raise ValueError(f"bootstrap status for {identity!r} requires state")
        state = data["state"]
        if not isinstance(state, str):
            raise TypeError(f"bootstrap state for {identity!r} must be a string")
        error = data.get("error") if "error" in data else None
        if error is not None and not isinstance(error, str):
            raise TypeError(f"bootstrap error for {identity!r} must be a string")
        restore = data.get("restore")
        return cls(
            identity=identity,
            state=IdentityBootstrapState.parse(state),
            error=error,
            restore=(
                IdentityRestoreProgress.from_dict(restore)
                if restore is not None
                else None
            ),
        )

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {"state": self.state.value}
        if self.error is not None:
            result["error"] = self.error
        if self.restore is not None:
            result["restore"] = self.restore.to_dict()
        return result


@dataclass(frozen=True)
class FirstBootPendingRow:
    """A row of the ``storage-migrate-native`` first-boot manifest that the
    gateway has not carried out yet.

    ``kind`` is ``"resume"`` (the identity resumes the session the migration
    recorded) or ``"fresh"`` (it starts on a fresh session). ``waiting_for``
    is ``"roster"`` (the identity list does not name the identity yet) or
    ``"carry_out"`` (it does, and the row is not carried out yet). Newer
    values are kept as sent.
    """

    identity: str
    kind: str
    waiting_for: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> FirstBootPendingRow:
        if not isinstance(data, dict):
            raise TypeError("first-boot pending row must be an object")
        for key in ("identity", "kind", "waiting_for"):
            if not isinstance(data.get(key), str):
                raise TypeError(f"first-boot pending row requires a string {key}")
        return cls(
            identity=data["identity"],
            kind=data["kind"],
            waiting_for=data["waiting_for"],
        )

    def to_dict(self) -> dict[str, Any]:
        return {
            "identity": self.identity,
            "kind": self.kind,
            "waiting_for": self.waiting_for,
        }


@dataclass(frozen=True)
class DeferredIdentity:
    """A roster identity a candidate launch left to the authoritative launch:
    waking it would change the stored roster.

    ``reason`` is ``"create_member"`` (the store holds no member for it),
    ``"move_role"`` (its stored member moves to another role),
    ``"fresh_session"`` (its stored session is gone),
    ``"first_boot_fresh"`` (a pending fresh first-boot row) or
    ``"resume_member"`` (a member would be spawned onto its stored session:
    Core holds no live member for it, or Meta 0.9.3 spawned its member).
    Newer values are kept as sent.
    """

    identity: str
    reason: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> DeferredIdentity:
        if not isinstance(data, dict):
            raise TypeError("deferred identity must be an object")
        for key in ("identity", "reason"):
            if not isinstance(data.get(key), str):
                raise TypeError(f"deferred identity requires a string {key}")
        return cls(identity=data["identity"], reason=data["reason"])

    def to_dict(self) -> dict[str, Any]:
        return {"identity": self.identity, "reason": self.reason}


@dataclass(frozen=True)
class IdentityBootstrapStatus:
    """Aggregate gateway bootstrap status returned by status/wait RPCs.

    ``first_boot_pending`` lists the first-boot manifest rows still to be
    carried out after ``storage-migrate-native``; it is empty when none is
    pending. ``deferred`` lists the identities a candidate launch left to the
    authoritative launch; they are not in ``identities`` and count neither as
    broken nor against ``ready``. It is empty on every other launch.
    """

    mode: IdentityBootstrapMode
    complete: bool
    ready: bool
    counts: IdentityBootstrapCounts
    identities: dict[str, IdentityBootstrapEntry]
    error: str | None = None
    timed_out: bool = False
    target: str | None = None
    startup_ready: bool | None = None
    first_boot_pending: tuple[FirstBootPendingRow, ...] = ()
    deferred: tuple[DeferredIdentity, ...] = ()

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> IdentityBootstrapStatus:
        if not isinstance(data, dict):
            raise TypeError("identity bootstrap status must be an object")
        mode = IdentityBootstrapMode.from_dict(data.get("mode", {}))
        counts = IdentityBootstrapCounts.from_dict(data.get("counts", {}))
        identities_raw = data.get("identities", {})
        if not isinstance(identities_raw, dict):
            raise TypeError("identity bootstrap identities must be an object")
        identities = {
            str(identity): IdentityBootstrapEntry.from_dict(
                str(identity), status
            )
            for identity, status in identities_raw.items()
        }
        target = data.get("target") if "target" in data else None
        if target is not None and not isinstance(target, str):
            raise TypeError("identity bootstrap target must be a string")
        error = data.get("error") if "error" in data else None
        if error is not None and not isinstance(error, str):
            raise TypeError("identity bootstrap error must be a string")
        pending_raw = data.get("first_boot_pending", [])
        if not isinstance(pending_raw, list):
            raise TypeError("identity bootstrap first_boot_pending must be a list")
        deferred_raw = data.get("deferred", [])
        if not isinstance(deferred_raw, list):
            raise TypeError("identity bootstrap deferred must be a list")
        return cls(
            mode=mode,
            complete=_required_bool(data, "complete"),
            ready=_required_bool(data, "ready"),
            error=error,
            counts=counts,
            identities=identities,
            timed_out=_optional_bool(
                data,
                "timed_out",
                default=False,
            ),
            target=target,
            startup_ready=_optional_bool(
                data,
                "startup_ready",
                default=None,
            ),
            first_boot_pending=tuple(
                FirstBootPendingRow.from_dict(row) for row in pending_raw
            ),
            deferred=tuple(
                DeferredIdentity.from_dict(row) for row in deferred_raw
            ),
        )

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {
            "mode": self.mode.to_dict(),
            "complete": self.complete,
            "ready": self.ready,
            "counts": self.counts.to_dict(),
            "identities": {
                identity: status.to_dict()
                for identity, status in self.identities.items()
            },
        }
        if self.error is not None:
            result["error"] = self.error
        if self.timed_out:
            result["timed_out"] = True
        if self.target is not None:
            result["target"] = self.target
        if self.startup_ready is not None:
            result["startup_ready"] = self.startup_ready
        if self.first_boot_pending:
            result["first_boot_pending"] = [
                row.to_dict() for row in self.first_boot_pending
            ]
        if self.deferred:
            result["deferred"] = [row.to_dict() for row in self.deferred]
        return result


def _validate_agent_identity(identity: str) -> str:
    if (
        not identity
        or identity.strip() != identity
        or any(ch.isspace() for ch in identity)
        or "/" in identity
    ):
        raise ValueError(f"invalid agent identity: {identity}")
    return identity


# ---------------------------------------------------------------------------
# Content blocks (REQ-43)
# ---------------------------------------------------------------------------

class ContentBlock:
    """Base for typed content blocks. Use from_dict() to deserialize."""

    @staticmethod
    def from_dict(data: dict[str, Any]) -> ContentBlock:
        t = data.get("type")
        if t == "text":
            return TextBlock(text=data["text"])
        elif t == "image":
            source = data.get("source", "inline")
            if source == "blob":
                return ImageBlock(
                    media_type=data["media_type"],
                    source="blob",
                    blob_id=data["blob_id"],
                )
            if source != "inline":
                raise ValueError("image source must be 'inline' or 'blob'")
            return ImageBlock(
                media_type=data["media_type"],
                data=data["data"],
                source="inline",
            )
        raise ValueError(f"unknown content block type: {t!r}")

    def to_dict(self) -> dict[str, Any]:
        raise NotImplementedError


@dataclass
class TextBlock(ContentBlock):
    text: str

    def to_dict(self) -> dict[str, Any]:
        return {"type": "text", "text": self.text}


@dataclass
class ImageBlock(ContentBlock):
    media_type: str
    data: str = ""
    source: str = "inline"
    blob_id: str | None = None

    def __post_init__(self) -> None:
        if self.source not in {"inline", "blob"}:
            raise ValueError("image source must be 'inline' or 'blob'")
        if self.source == "blob" and not self.blob_id:
            raise ValueError("blob image blocks require blob_id")

    def to_dict(self) -> dict[str, Any]:
        if self.source == "blob":
            return {
                "type": "image",
                "media_type": self.media_type,
                "source": "blob",
                "blob_id": self.blob_id,
            }
        return {
            "type": "image",
            "media_type": self.media_type,
            "source": "inline",
            "data": self.data,
        }


# ---------------------------------------------------------------------------
# DurableAgentSpec (REQ-40)
# ---------------------------------------------------------------------------

# Meerkat's `MobRuntimeMode` wire vocabulary (serde snake_case).
_VALID_RUNTIME_MODES = frozenset({"autonomous_host", "turn_driven"})


def _content_input_to_wire(content: str | list[ContentBlock]) -> str | list[dict[str, Any]]:
    """Serialize a Rust ``ContentInput`` (untagged: text or block list)."""
    if isinstance(content, str):
        return content
    return [block.to_dict() for block in content]


def _content_input_from_wire(raw: Any) -> str | list[ContentBlock]:
    if isinstance(raw, str):
        return raw
    if isinstance(raw, list):
        return [ContentBlock.from_dict(block) for block in raw]
    raise TypeError(
        "content input must be a string or a list of content blocks, "
        f"got {type(raw).__name__}"
    )


@dataclass
class DurableAgentSpec:
    """Identity-first agent specification matching Rust DurableAgentSpec.

    ``runtime_mode_override`` pins meerkat's runtime mode for this identity
    (``"autonomous_host"`` or ``"turn_driven"``); ``None`` defers to the
    profile's ``runtime_mode`` in mob.toml, whose meerkat default is
    ``autonomous_host``. ``initial_message`` is the first turn a fresh
    ``autonomous_host`` member runs on (plain text or content blocks, Rust
    ``ContentInput``); ``None`` leaves meerkat's fallback spawn prompt in
    place. Both fields are omitted from the wire when unset, so a roster that
    never sets them is byte-identical to earlier SDKs.
    """

    identity: str
    profile: str
    addressability: str = "addressable"
    display_name: str | None = None
    labels: dict[str, str] = field(default_factory=dict)
    context: Any | None = None
    additional_instructions: list[str] = field(default_factory=list)
    # Exact canonical Meerkat host ref. Callers must never reinterpret an
    # unavailable selected host as local placement.
    placement: str | None = None
    runtime_mode_override: str | None = None
    initial_message: str | list[ContentBlock] | None = None

    def __post_init__(self) -> None:
        self.identity = _validate_agent_identity(self.identity)
        if (
            self.runtime_mode_override is not None
            and self.runtime_mode_override not in _VALID_RUNTIME_MODES
        ):
            raise ValueError(
                "runtime_mode_override must be one of "
                f"{sorted(_VALID_RUNTIME_MODES)}, got {self.runtime_mode_override!r}"
            )
        if self.initial_message is not None and not (
            isinstance(self.initial_message, str)
            or (
                isinstance(self.initial_message, list)
                and all(isinstance(b, ContentBlock) for b in self.initial_message)
            )
        ):
            raise TypeError(
                "initial_message must be a string or a list of ContentBlock, "
                f"got {type(self.initial_message).__name__}"
            )

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {
            "identity": self.identity,
            "profile": self.profile,
            "addressability": self.addressability,
        }
        if self.display_name is not None:
            result["display_name"] = self.display_name
        result["labels"] = dict(self.labels)
        if self.context is not None:
            result["context"] = self.context
        result["additional_instructions"] = list(self.additional_instructions)
        if self.placement is not None:
            result["placement"] = self.placement
        if self.runtime_mode_override is not None:
            result["runtime_mode_override"] = self.runtime_mode_override
        if self.initial_message is not None:
            result["initial_message"] = _content_input_to_wire(self.initial_message)
        return result

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> DurableAgentSpec:
        raw_initial = data.get("initial_message")
        return cls(
            identity=data["identity"],
            profile=data["profile"],
            addressability=data.get("addressability", "addressable"),
            display_name=data.get("display_name"),
            labels=dict(data.get("labels") or {}),
            context=data.get("context"),
            additional_instructions=list(data.get("additional_instructions") or []),
            placement=data.get("placement"),
            runtime_mode_override=data.get("runtime_mode_override"),
            initial_message=(
                None if raw_initial is None else _content_input_from_wire(raw_initial)
            ),
        )


@dataclass
class RoleMigrationDeclaration:
    """A host assertion that one identity's durable member role changed.

    A durable member whose role changed refuses to resume until the host names
    it here, which is deliberate: an unintended role edit must not silently
    restamp a member's durable role, comms name and binding.

    Boot-scoped. It is read once into the gateway's session bridge, never
    persisted, and gone the moment it is absent from the next boot payload.
    Meerkat re-verifies ``from_role`` against durable state, so a mistyped
    value refuses rather than restamps, and it ignores the declaration entirely
    once the roles already agree - so leaving a completed migration in place is
    inert, not a repeat restamp.
    """

    identity: str
    from_role: str

    def __post_init__(self) -> None:
        self.identity = _validate_agent_identity(self.identity)
        if not self.from_role or self.from_role.strip() != self.from_role:
            raise ValueError(f"invalid predecessor role: {self.from_role}")

    def to_dict(self) -> dict[str, Any]:
        return {"identity": self.identity, "from_role": self.from_role}

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> RoleMigrationDeclaration:
        return cls(identity=data["identity"], from_role=data["from_role"])


# ---------------------------------------------------------------------------
# DispatchInput (REQ-43)
# ---------------------------------------------------------------------------

@dataclass
class DispatchInput:
    """Typed dispatch input matching Rust DispatchInput."""

    content: str | list[ContentBlock]
    origin: str
    correlation_id: str | None = None
    idempotency_key: str | None = None

    def __post_init__(self) -> None:
        if self.origin not in _VALID_ORIGINS:
            raise ValueError(
                f"origin must be one of {sorted(_VALID_ORIGINS)}, got {self.origin!r}"
            )

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {"origin": self.origin}
        if isinstance(self.content, str):
            result["content"] = self.content
        else:
            result["content"] = [b.to_dict() for b in self.content]
        if self.correlation_id is not None:
            result["correlation_id"] = self.correlation_id
        if self.idempotency_key is not None:
            result["idempotency_key"] = self.idempotency_key
        return result

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> DispatchInput:
        raw_content = data["content"]
        if isinstance(raw_content, str):
            content: str | list[ContentBlock] = raw_content
        else:
            content = [ContentBlock.from_dict(b) for b in raw_content]
        return cls(
            content=content,
            origin=data["origin"],
            correlation_id=data.get("correlation_id"),
            idempotency_key=data.get("idempotency_key"),
        )

    @classmethod
    def system(cls, content: str) -> DispatchInput:
        """System-origin dispatch from plain text."""
        return cls(content=content, origin="system")

    @classmethod
    def connector(cls, content: str, *, correlation_id: str | None = None) -> DispatchInput:
        """Connector-origin dispatch."""
        return cls(content=content, origin="connector", correlation_id=correlation_id)

    @classmethod
    def scheduler(cls, content: str) -> DispatchInput:
        """Scheduler-origin dispatch."""
        return cls(content=content, origin="scheduler")


# ---------------------------------------------------------------------------
# ManagedPeerEdge (REQ-43a)
# ---------------------------------------------------------------------------

@dataclass
class ManagedPeerEdge:
    """A managed topology edge between two agent identities."""

    a: str
    b: str

    def __post_init__(self) -> None:
        a = _validate_agent_identity(self.a)
        b = _validate_agent_identity(self.b)
        if a == b:
            raise ValueError(f"managed peer edge cannot connect an identity to itself: {a}")
        if b < a:
            a, b = b, a
        self.a = a
        self.b = b

    def to_dict(self) -> dict[str, Any]:
        return {"a": self.a, "b": self.b}

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ManagedPeerEdge:
        return cls(a=data["a"], b=data["b"])


# ---------------------------------------------------------------------------
# ExternalToolDef, AgentBuildContext, AgentBuildDraft (REQ-43a)
# ---------------------------------------------------------------------------

@dataclass
class ExternalToolDef:
    """Tool definition for the customizer boundary."""

    name: str
    description: str
    input_schema: dict[str, Any]
    #: Installed artifact this tool's handler came from; verified by the
    #: gateway against the host's installed artifacts. Attribution only.
    artifact: ToolArtifactRef | None = None

    def to_dict(self) -> dict[str, Any]:
        out: dict[str, Any] = {
            "name": self.name,
            "description": self.description,
            "input_schema": self.input_schema,
        }
        if self.artifact is not None:
            out["artifact"] = self.artifact.to_wire()
        return out

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ExternalToolDef:
        artifact = data.get("artifact")
        return cls(
            name=data["name"],
            description=data["description"],
            input_schema=data["input_schema"],
            artifact=ToolArtifactRef.from_wire(artifact) if artifact is not None else None,
        )


@dataclass(frozen=True)
class AgentBuildContext:
    """Read-only context provided to AgentCustomizer at build time."""

    identity: str
    active_peers: list[str]
    managed_edges: list[ManagedPeerEdge]

    def to_dict(self) -> dict[str, Any]:
        return {
            "identity": self.identity,
            "active_peers": list(self.active_peers),
            "managed_edges": [e.to_dict() for e in self.managed_edges],
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> AgentBuildContext:
        return cls(
            identity=data["identity"],
            active_peers=list(data.get("active_peers") or []),
            managed_edges=[
                ManagedPeerEdge.from_dict(e)
                for e in (data.get("managed_edges") or [])
            ],
        )


@dataclass(frozen=True)
class RosterContext:
    """Read-only context the gateway hands to ``RosterProviderProtocol.roster``.

    Mirrors the Rust ``RosterContext`` one for one; it arrives on the wire as
    ``callback/roster_provider/roster`` ``params["context"]``.

    ``mob_definition`` is the booted mob's compiled definition as a plain
    JSON object (``[mob]`` id, profiles, ...). It is ``None`` only when the
    runtime resolving the roster has no mob definition, which the bundled
    ``rpc_gateway`` never does; the field is optional because in-process Rust
    embedders and test harnesses can call the provider without one.

    ``previous_identities`` is call-site-dependent prior-identity context,
    not an authoritative membership snapshot on every callback. Topology
    queries and edge reconciliation supply currently registered identities.
    Bootstrap, full roster refresh/reconcile, and reset-time spec adoption
    currently pass an empty list, even if identities are already registered.
    """

    mob_definition: dict[str, Any] | None = None
    previous_identities: list[str] = field(default_factory=list)

    def to_dict(self) -> dict[str, Any]:
        return {
            "mob_definition": self.mob_definition,
            "previous_identities": list(self.previous_identities),
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> RosterContext:
        definition = data.get("mob_definition")
        if definition is not None and not isinstance(definition, dict):
            raise ValueError("RosterContext.mob_definition must be an object or null")
        return cls(
            mob_definition=definition,
            previous_identities=[str(i) for i in (data.get("previous_identities") or [])],
        )


@dataclass
class AgentBuildDraft:
    """Mutable draft that AgentCustomizer modifies."""

    model: str | None = None
    system_prompt: str | None = None
    additional_instructions: list[str] = field(default_factory=list)
    labels: dict[str, str] = field(default_factory=dict)
    app_context: Any | None = None
    external_tools: list[ExternalToolDef] = field(default_factory=list)
    #: Per-identity provider parameter overrides, carried verbatim to the
    #: gateway. Mirrors meerkat's ``ProviderParamsOverride`` (the type behind
    #: ``AgentBuildConfig.provider_params``): provider knobs with no Meta
    #: vocabulary of their own, such as OpenAI ``prompt_cache_key`` /
    #: ``prompt_cache_options`` / ``prompt_cache_retention`` and Anthropic
    #: ``cache_control``. Kept as a plain mapping here because the Rust
    #: boundary parses it fail-closed — an unknown or mistyped knob is
    #: rejected there rather than dropped.
    provider_params: dict[str, Any] | None = None
    _tool_handlers: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)

    def register_tool(
        self,
        name: str,
        handler: Any,
        *,
        description: str = "",
        input_schema: dict[str, Any] | None = None,
        artifact: ToolArtifactRef | None = None,
    ) -> None:
        """Register a callable external tool on this build.

        Parity with :meth:`SessionBuildOptions.register_tool`, but available in
        ``AgentCustomizer.customize_build`` — which runs on BOTH fresh create and
        restore/reconcile. The handler is dispatched in-process when the agent
        invokes the tool; it receives a dict of arguments and returns a
        JSON-serializable result. Use this to reattach identity-scoped tools
        (MCP, comms, etc.) so resumed agents keep them.

        Args:
            name: Tool name (string).
            handler: Async or sync callable ``(args: dict) -> Any``.
            description: Human-readable tool description.
            input_schema: JSON Schema for the tool arguments
                (defaults to ``{"type": "object"}``).
            artifact: Optional installed artifact this handler came from,
                verified by the gateway against the host's installed artifacts.
        """
        if not isinstance(name, str):
            raise TypeError(f"tool name must be a string, got {type(name).__name__}: {name!r}")
        if not callable(handler):
            raise TypeError(f"handler must be callable, got {type(handler).__name__}: {handler!r}")
        if artifact is not None and not isinstance(artifact, ToolArtifactRef):
            raise TypeError(
                f"artifact must be ToolArtifactRef, got {type(artifact).__name__}: {artifact!r}"
            )
        # Registering a name again replaces its definition wholesale: a
        # registration without an artifact never inherits the previous one's.
        self.external_tools = [tool for tool in self.external_tools if tool.name != name]
        self.external_tools.append(
            ExternalToolDef(
                name=name,
                description=description,
                input_schema=input_schema if input_schema is not None else {"type": "object"},
                artifact=artifact,
            )
        )
        self._tool_handlers[name] = handler

    @property
    def tool_handlers(self) -> dict[str, Any]:
        """Handlers registered via :meth:`register_tool` (in-process only)."""
        return dict(self._tool_handlers)

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {}
        if self.model is not None:
            result["model"] = self.model
        if self.system_prompt is not None:
            result["system_prompt"] = self.system_prompt
        result["additional_instructions"] = list(self.additional_instructions)
        result["labels"] = dict(self.labels)
        if self.app_context is not None:
            result["app_context"] = self.app_context
        result["external_tools"] = [t.to_dict() for t in self.external_tools]
        # The gateway replaces the draft wholesale with what it gets back, so
        # a field omitted here is a field cleared on every customized build.
        if self.provider_params is not None:
            result["provider_params"] = self.provider_params
        return result

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> AgentBuildDraft:
        return cls(
            model=data.get("model"),
            system_prompt=data.get("system_prompt"),
            additional_instructions=list(data.get("additional_instructions") or []),
            labels=dict(data.get("labels") or {}),
            app_context=data.get("app_context"),
            external_tools=[
                ExternalToolDef.from_dict(t)
                for t in (data.get("external_tools") or [])
            ],
            provider_params=data.get("provider_params"),
        )


# ---------------------------------------------------------------------------
# IdentityStatus + supporting types (REQ-43b)
# ---------------------------------------------------------------------------

@dataclass(frozen=True)
class WakeRefusal:
    """Why the identity's last wake was refused; cleared by the next successful wake.

    ``code`` is ``-32051`` (``IDENTITY_UNRESUMABLE_CODE``) for a final refusal,
    when the identity reads ``broken``, or ``-32603`` for a refusal held until
    an operator acts, when it reads ``dormant``. ``session_id`` is the session
    whose resume was refused. ``remedy`` names the way out:
    ``"delete_identity"``, ``"repair_session"``, ``"resolve_role_migration"``
    or ``"operator_review"``; a newer value is kept as sent.
    """

    code: int
    session_id: str
    remedy: str

    def to_dict(self) -> dict[str, Any]:
        return {"code": self.code, "session_id": self.session_id, "remedy": self.remedy}

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> WakeRefusal:
        code = data.get("code")
        return cls(
            code=code if isinstance(code, int) else 0,
            session_id=str(data.get("session_id", "")),
            remedy=str(data.get("remedy", "")),
        )


@dataclass(frozen=True)
class ReplacedSession:
    """The session the last wake replaced with a fresh spawn.

    Core held no durable session for the identity's last session
    (``reason`` ``"absent"``; a newer reason is kept as sent), so the wake
    started the identity fresh. Present until the next wake.
    """

    session_id: str
    reason: str

    def to_dict(self) -> dict[str, Any]:
        return {"session_id": self.session_id, "reason": self.reason}

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ReplacedSession:
        return cls(
            session_id=str(data.get("session_id", "")),
            reason=str(data.get("reason", "")),
        )


@dataclass(frozen=True)
class IdentityConvergence:
    """Core's convergence status for an identity a host intent declaration manages.

    The fields are Core's, unchanged: ``condition`` and ``decision`` are
    Core's snake_case values (for example ``"converged"`` or ``"backoff"``)
    and are kept as sent.
    """

    agent_identity: str
    condition: str
    observed_at_ms: int
    desired_intent_revision: int | None = None
    active_intent_revision: int | None = None
    decision: str | None = None
    detail: str | None = None

    def to_dict(self) -> dict[str, Any]:
        return {
            "agent_identity": self.agent_identity,
            "desired_intent_revision": self.desired_intent_revision,
            "active_intent_revision": self.active_intent_revision,
            "decision": self.decision,
            "condition": self.condition,
            "observed_at_ms": self.observed_at_ms,
            "detail": self.detail,
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> IdentityConvergence:
        observed_at_ms = data.get("observed_at_ms")
        return cls(
            agent_identity=str(data.get("agent_identity", "")),
            condition=str(data.get("condition", "")),
            observed_at_ms=observed_at_ms if isinstance(observed_at_ms, int) else 0,
            desired_intent_revision=data.get("desired_intent_revision"),
            active_intent_revision=data.get("active_intent_revision"),
            decision=data.get("decision"),
            detail=data.get("detail"),
        )


def _view_part(data: dict[str, Any], key: str, part: Any) -> Any:
    """Parse one optional identity-view object (``convergence``,
    ``wake_refusal`` or ``replaced_session``); ``None`` when it is absent."""
    raw = data.get(key)
    return part.from_dict(raw) if isinstance(raw, dict) else None


@dataclass(frozen=True)
class IdentityStatus:
    """Full identity status (TYPE-18).

    Core's mob machine holds the identity's session and generation; the
    status reads them per call. ``state`` is ``"active"`` (Core holds a live
    member), ``"dormant"`` (it holds none) or ``"broken"``: it holds none and
    the last wake was refused for good (see :attr:`wake_refusal`), or it
    holds the identity's live member Broken because it could not restore it
    (``agent_runtime_id`` is then set and there is no ``wake_refusal``).
    ``agent_runtime_id`` is ``None`` when the identity is not live;
    ``session_id`` and ``generation`` are ``None`` when Core holds no binding
    for it.
    """

    identity: str
    state: str
    agent_runtime_id: str | None = None
    session_id: str | None = None
    profile: str | None = None
    #: ``"autonomous_host"`` or ``"turn_driven"``.
    runtime_mode: str | None = None
    addressability: str = "addressable"
    display_name: str | None = None
    labels: dict[str, str] = field(default_factory=dict)
    generation: int | None = None
    #: ``{"reason", "artifact_refusal"?}`` while a restored member's customizer
    #: tools are not published yet.
    customizer_tools_pending: dict[str, Any] | None = None
    #: ``True`` when a host intent declaration in Core manages the identity;
    #: Meta's lifecycle verbs then refuse it with
    #: :class:`~meerkat_meta.errors.IdentityHostAdoptedError`.
    intent_managed: bool = False
    #: Core's convergence status, when :attr:`intent_managed` is set and Core
    #: could read it.
    convergence: IdentityConvergence | None = None
    #: The last refused wake, until a wake succeeds.
    wake_refusal: WakeRefusal | None = None
    #: The session the last wake replaced with a fresh spawn, until the next
    #: wake.
    replaced_session: ReplacedSession | None = None
    #: The exact delivery scope to persist before a scoped dispatch
    #: (``dispatch(..., expected_scope=)``); ``None`` when none can be
    #: captured, with the reason in :attr:`delivery_scope_unavailable`.
    delivery_scope: DeliveryScope | None = None
    #: ``{"kind", "reason"}`` when :attr:`delivery_scope` is ``None``.
    delivery_scope_unavailable: dict[str, Any] | None = None
    #: Core's mob member id of the identity's last binding: the roster
    #: member id, Meta's comms-safe encoding of the identity (the identity
    #: itself when it is already a valid comms name, ``mk--...`` otherwise,
    #: as for an identity with a ``:``), from the same binding as
    #: :attr:`session_id`; ``None`` when Core holds no binding for the
    #: identity.
    member_id: str | None = None

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {
            "identity": self.identity,
            "state": self.state,
            "addressability": self.addressability,
            "labels": dict(self.labels),
            "intent_managed": self.intent_managed,
        }
        if self.agent_runtime_id is not None:
            result["agent_runtime_id"] = self.agent_runtime_id
        if self.session_id is not None:
            result["session_id"] = self.session_id
        if self.member_id is not None:
            result["member_id"] = self.member_id
        if self.profile is not None:
            result["profile"] = self.profile
        if self.runtime_mode is not None:
            result["runtime_mode"] = self.runtime_mode
        if self.display_name is not None:
            result["display_name"] = self.display_name
        if self.generation is not None:
            result["generation"] = self.generation
        if self.customizer_tools_pending is not None:
            result["customizer_tools_pending"] = dict(self.customizer_tools_pending)
        if self.convergence is not None:
            result["convergence"] = self.convergence.to_dict()
        if self.wake_refusal is not None:
            result["wake_refusal"] = self.wake_refusal.to_dict()
        if self.replaced_session is not None:
            result["replaced_session"] = self.replaced_session.to_dict()
        if self.delivery_scope is not None:
            result["delivery_scope"] = self.delivery_scope.to_dict()
        if self.delivery_scope_unavailable is not None:
            result["delivery_scope_unavailable"] = dict(self.delivery_scope_unavailable)
        return result

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> IdentityStatus:
        delivery_scope: DeliveryScope | None = None
        unavailable_raw = data.get("delivery_scope_unavailable")
        delivery_scope_unavailable = (
            dict(unavailable_raw) if isinstance(unavailable_raw, dict) else None
        )
        scope_raw = data.get("delivery_scope")
        if isinstance(scope_raw, dict):
            try:
                delivery_scope = DeliveryScope.from_dict(scope_raw)
            except ContractMismatchError as exc:
                # A scope this SDK cannot read is never guessed at; the rest
                # of the status stays usable.
                delivery_scope_unavailable = {
                    "kind": "unsupported_delivery_scope_version",
                    "reason": str(exc),
                }
        pending_raw = data.get("customizer_tools_pending")
        # `.get()` with defaults to match the TS parser's graceful degradation.
        return cls(
            identity=str(data.get("identity", "")),
            state=str(data.get("state", "")),
            agent_runtime_id=data.get("agent_runtime_id"),
            session_id=data.get("session_id"),
            member_id=data.get("member_id"),
            profile=data.get("profile"),
            runtime_mode=data.get("runtime_mode"),
            addressability=data.get("addressability", "addressable"),
            display_name=data.get("display_name"),
            labels=dict(data.get("labels") or {}),
            generation=data.get("generation"),
            customizer_tools_pending=(
                dict(pending_raw) if isinstance(pending_raw, dict) else None
            ),
            intent_managed=data.get("intent_managed") is True,
            convergence=_view_part(data, "convergence", IdentityConvergence),
            wake_refusal=_view_part(data, "wake_refusal", WakeRefusal),
            replaced_session=_view_part(data, "replaced_session", ReplacedSession),
            delivery_scope=delivery_scope,
            delivery_scope_unavailable=delivery_scope_unavailable,
        )


# ---------------------------------------------------------------------------
# Typed return models
# ---------------------------------------------------------------------------


class CompletionProgress(str, Enum):
    """How an observed cursor relates to a baseline captured at dispatch."""

    PENDING = "pending"
    COMPLETED = "completed"
    INCARNATION_CHANGED = "incarnation_changed"


@dataclass(frozen=True)
class CompletionCursor:
    """Comparable completion identity for one identity's stream of turns.

    Waiting for an agent's next answer must never compare output TEXT: two
    consecutive turns can legitimately produce byte-identical output, and a
    text comparison then reports "no new turn" for the whole configured wait.
    This cursor is the comparable atom instead — never derived from output
    content, a content hash, a timestamp, or a per-poll uuid.

    ``epoch`` is Core's fence for the identity's live member (the runtime
    incarnation this count belongs to); ``turns`` counts turns observed as
    completed within it. Turn counts are not comparable across
    incarnations, so classify with :meth:`progress_since` rather than
    comparing ``turns`` directly.
    """

    epoch: int = 0
    turns: int = 0

    def to_dict(self) -> dict[str, int]:
        return {"epoch": self.epoch, "turns": self.turns}

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> CompletionCursor:
        return cls(
            epoch=int(data.get("epoch", 0)),
            turns=int(data.get("turns", 0)),
        )

    def progress_since(self, baseline: CompletionCursor) -> CompletionProgress:
        """Classify this cursor against a baseline captured before delivery."""
        if self.epoch != baseline.epoch:
            return CompletionProgress.INCARNATION_CHANGED
        if self.turns > baseline.turns:
            return CompletionProgress.COMPLETED
        return CompletionProgress.PENDING


def _completion_cursor_from(data: dict[str, Any], key: str) -> CompletionCursor | None:
    """Read an optional cursor field. Absent (older gateway) stays ``None``."""
    raw = data.get(key)
    return CompletionCursor.from_dict(raw) if isinstance(raw, dict) else None


def _turn_ticket_from(data: dict[str, Any]) -> str | None:
    """Read the ``turn.ticket`` a tracked delivery returned, if any."""
    turn = data.get("turn")
    ticket = turn.get("ticket") if isinstance(turn, dict) else None
    return ticket if isinstance(ticket, str) else None


@dataclass(frozen=True)
class TurnUnavailable:
    """Why a delivery that asked for ``track_turn`` could not be tracked, and
    whether it was delivered at all.

    ``code`` is stable. Every code but ``not_delivered`` means the delivery
    happened, exactly once, untracked: ``autonomous_host`` (the member runs
    in ``autonomous_host`` mode, read from the runtime's live roster entry;
    its inbox delivery reports no per-turn completion), ``runtime_refused``
    (the runtime refused per-turn completion for the member's live mode for
    another reason), ``externally_bound``, ``host_human_input``,
    ``bridge_cannot_report_output`` or ``session_rotated`` (a re-dispatch that
    landed on a new session after a repair and ran as its own turn). ``not_delivered`` means NOTHING was
    delivered (no session bridge or no bound runtime), so there is no turn to
    wait for. ``delivered`` says which; ``reason`` is the human-readable
    detail.
    """

    code: str
    reason: str
    delivered: bool = True

    def to_dict(self) -> dict[str, Any]:
        return {"code": self.code, "reason": self.reason, "delivered": self.delivered}

    @classmethod
    def from_wire(cls, raw: Any) -> TurnUnavailable | None:
        if isinstance(raw, dict):
            code = str(raw.get("code", ""))
            delivered = raw.get("delivered")
            return cls(
                code=code,
                reason=str(raw.get("reason", "")),
                delivered=(
                    delivered if isinstance(delivered, bool) else code != "not_delivered"
                ),
            )
        return None


def _turn_tracking_to_dict(
    result: dict[str, Any],
    turn_ticket: str | None,
    turn_unavailable: TurnUnavailable | None,
) -> None:
    if turn_ticket is not None:
        result["turn"] = {"ticket": turn_ticket}
    elif turn_unavailable is not None:
        result["turn"] = None
        result["turn_unavailable"] = turn_unavailable.to_dict()


@dataclass(frozen=True)
class RegisteredSpec:
    """An identity's registered roster row (``meta/registered_spec``).

    ``spec`` is the full row as the gateway holds it, including
    ``additional_instructions``; ``spec_digest`` is its canonical digest,
    which a send passes back as ``expected_spec_digest``.
    """

    identity: str
    spec: dict[str, Any]
    spec_digest: str

    @property
    def additional_instructions(self) -> list[str]:
        return list(self.spec.get("additional_instructions") or [])

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> RegisteredSpec:
        return cls(
            identity=str(data.get("identity", "")),
            spec=dict(data.get("spec") or {}),
            spec_digest=str(data["spec_digest"]),
        )


@dataclass(frozen=True)
class SendResult:
    """Typed result from identity-first send()."""

    fencing_token: int
    #: Cursor read before delivery: an IDENTITY-WIDE barrier. Another
    #: delivery's completion also passes it, so it does not name this turn;
    #: use :attr:`turn_ticket` for that. ``None`` when the gateway predates
    #: the field.
    completion_baseline: CompletionCursor | None = None
    #: The ticket naming THIS turn, when the send asked for ``track_turn`` and
    #: the gateway could track it. Wait on it with ``wait_for_turn``.
    turn_ticket: str | None = None
    #: Why a requested ticket is unavailable (the turn was still delivered).
    turn_unavailable: TurnUnavailable | None = None

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {"fencing_token": self.fencing_token}
        if self.completion_baseline is not None:
            result["completion_baseline"] = self.completion_baseline.to_dict()
        _turn_tracking_to_dict(result, self.turn_ticket, self.turn_unavailable)
        return result

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> SendResult:
        return cls(
            fencing_token=int(data.get("fencing_token", 0)),
            completion_baseline=_completion_cursor_from(data, "completion_baseline"),
            turn_ticket=_turn_ticket_from(data),
            turn_unavailable=TurnUnavailable.from_wire(data.get("turn_unavailable")),
        )


def _optional_text(data: dict[str, Any], key: str) -> str | None:
    value = data.get(key)
    return value if isinstance(value, str) else None


@dataclass(frozen=True)
class ContinuationStatus:
    """Where a keyed send's delivery stands, read for each answer.

    ``state`` is one of:

    - ``pending``: committed, not admitted yet. Replay the same key to read
      it again. A ``pending`` that survives a gateway restart is not proof of
      loss: do not resend it under a new key.
    - ``admitted``: queued in the member's session; ``session_id`` and
      ``input_id`` name the input, which :meth:`MetaRuntime.send_status`
      follows. Not proof that a turn ran.
    - ``stranded``: final. The content was never admitted: the member
      incarnation it was bound to was replaced. Resend under a new key.
    - ``refused``: final, never admitted; ``reason`` says why.

    Tolerate other values: the vocabulary may grow.
    """

    state: str
    session_id: str | None = None
    input_id: str | None = None
    reason: str | None = None

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {"state": self.state}
        if self.session_id is not None:
            result["session_id"] = self.session_id
        if self.input_id is not None:
            result["input_id"] = self.input_id
        if self.reason is not None:
            result["reason"] = self.reason
        return result

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ContinuationStatus:
        return cls(
            state=str(data.get("state", "")),
            session_id=_optional_text(data, "session_id"),
            input_id=_optional_text(data, "input_id"),
            reason=_optional_text(data, "reason"),
        )


@dataclass(frozen=True)
class KeyedSendResult:
    """Typed result of a keyed send (``send(..., idempotency_key=...)``): the
    member continuation the content was committed as.

    A replay of the key answers the same ``delivery_id`` and
    ``committed_at_ms``; ``status`` is read for each answer and may advance.
    """

    delivery_id: str
    committed_at_ms: int
    status: ContinuationStatus

    def to_dict(self) -> dict[str, Any]:
        return {
            "continuation": {
                "delivery_id": self.delivery_id,
                "committed_at_ms": self.committed_at_ms,
                "status": self.status.to_dict(),
            }
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> KeyedSendResult:
        continuation = data.get("continuation")
        if not isinstance(continuation, dict) or not isinstance(
            continuation.get("delivery_id"), str
        ):
            raise ValueError("a keyed meta/send result carries no continuation receipt")
        status = continuation.get("status")
        return cls(
            delivery_id=continuation["delivery_id"],
            committed_at_ms=int(continuation.get("committed_at_ms", 0)),
            status=ContinuationStatus.from_dict(status if isinstance(status, dict) else {}),
        )


@dataclass(frozen=True)
class InputState:
    """The runtime's state of one input, as meerkat's ``session/input_status``
    reports it.

    ``current_state`` is ``accepted``, ``queued``, ``staged``, ``applied``,
    ``applied_pending_consumption``, ``consumed``, ``superseded``,
    ``coalesced`` or ``abandoned``; ``terminal_outcome`` is set once the input
    is terminal (``completed``, ``abandoned``, ``superseded``, ``coalesced``,
    ``cancelled``). Tolerate other values. ``raw`` is the whole wire object.
    """

    input_id: str
    current_state: str
    terminal_outcome: str | None = None
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> InputState:
        return cls(
            input_id=str(data.get("input_id", "")),
            current_state=str(data.get("current_state", "")),
            terminal_outcome=_optional_text(data, "terminal_outcome"),
            raw=copy.deepcopy(data),
        )


@dataclass(frozen=True)
class SendStatusResult:
    """Typed result of :meth:`MetaRuntime.send_status`: ``state`` is ``None``
    when the session holds no such input."""

    state: InputState | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> SendStatusResult:
        state = data.get("state")
        return cls(state=InputState.from_dict(state) if isinstance(state, dict) else None)


@dataclass(frozen=True)
class DispatchResult:
    """Typed result from identity-first dispatch()."""

    fencing_token: int
    durable: bool
    #: See :attr:`SendResult.completion_baseline`.
    completion_baseline: CompletionCursor | None = None
    #: See :attr:`SendResult.turn_ticket`.
    turn_ticket: str | None = None
    #: See :attr:`SendResult.turn_unavailable`.
    turn_unavailable: TurnUnavailable | None = None

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {
            "fencing_token": self.fencing_token,
            "durable": self.durable,
        }
        if self.completion_baseline is not None:
            result["completion_baseline"] = self.completion_baseline.to_dict()
        _turn_tracking_to_dict(result, self.turn_ticket, self.turn_unavailable)
        return result

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> DispatchResult:
        return cls(
            fencing_token=int(data.get("fencing_token", 0)),
            durable=bool(data.get("durable", False)),
            completion_baseline=_completion_cursor_from(data, "completion_baseline"),
            turn_ticket=_turn_ticket_from(data),
            turn_unavailable=TurnUnavailable.from_wire(data.get("turn_unavailable")),
        )


#: Serialized format version of :class:`DeliveryScope` this SDK reads.
DELIVERY_SCOPE_VERSION = 1


@dataclass(frozen=True)
class DeliveryScope:
    """One identity's exact delivery scope, from :meth:`status`.

    Persist it (``to_dict()``) before a scoped dispatch, and pass the same
    scope to ``dispatch(..., expected_scope=)`` and ``recover_delivery``. It
    pins the identity's live member as Core holds it (runtime id, Core's
    member generation, and Core's fence token in ``lease_fencing_token``)
    and meerkat's native member scope (``member``, opaque and versioned on
    its own). It is a selector and stale-binding guard: the gateway
    re-validates every atom and refuses a moved scope with
    :class:`~meerkat_meta.errors.StaleScopeError`. A scope persisted by Meta
    0.9.3 holds Meta's own atoms and is refused as stale.
    """

    identity: str
    agent_runtime_id: str
    generation: int
    lease_fencing_token: int
    member: dict[str, Any] = field(compare=True, hash=False)
    version: int = DELIVERY_SCOPE_VERSION

    @property
    def session_id(self) -> str | None:
        """The member session the scope pins."""
        session_id = self.member.get("session_id")
        return session_id if isinstance(session_id, str) else None

    def to_dict(self) -> dict[str, Any]:
        """The exact persisted form the gateway expects back."""
        return {
            "version": self.version,
            "identity": self.identity,
            "agent_runtime_id": self.agent_runtime_id,
            "generation": self.generation,
            "lease_fencing_token": self.lease_fencing_token,
            "member": copy.deepcopy(self.member),
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> DeliveryScope:
        """Decode a persisted scope.

        Raises :class:`~meerkat_meta.errors.ContractMismatchError` for a
        scope version this SDK does not read and :class:`ValueError` for a
        malformed scope; neither is ever read as a guess.
        """
        if not isinstance(data, dict):
            raise ValueError("delivery scope must be an object")
        version = data.get("version")
        if isinstance(version, bool) or not isinstance(version, int):
            raise ValueError("delivery scope has no integer version")
        if version != DELIVERY_SCOPE_VERSION:
            raise ContractMismatchError(
                f"unsupported delivery scope version {version} "
                f"(this SDK reads version {DELIVERY_SCOPE_VERSION})"
            )
        identity = data.get("identity")
        runtime_id = data.get("agent_runtime_id")
        generation = data.get("generation")
        token = data.get("lease_fencing_token")
        member = data.get("member")
        if not isinstance(identity, str) or not identity:
            raise ValueError("delivery scope identity must be a non-empty string")
        if not isinstance(runtime_id, str) or not runtime_id:
            raise ValueError("delivery scope agent_runtime_id must be a non-empty string")
        for name, value in (("generation", generation), ("lease_fencing_token", token)):
            if isinstance(value, bool) or not isinstance(value, int) or value < 0:
                raise ValueError(f"delivery scope {name} must be a non-negative integer")
        if not isinstance(member, dict):
            raise ValueError("delivery scope member must be an object")
        return cls(
            identity=identity,
            agent_runtime_id=runtime_id,
            generation=generation,
            lease_fencing_token=token,
            member=copy.deepcopy(member),
            version=version,
        )


@dataclass(frozen=True)
class DeliveryReceipt:
    """What a scoped dispatch's admission proved.

    ``stage`` is the admission stage the receipt proves (``ingress_accepted``
    for this path: the member's work lane accepted the delivery; it is not a
    durable runtime-input claim). ``session_id`` is the scope's session,
    validated inside meerkat's admission.
    """

    work_ref: str
    stage: str
    session_id: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> DeliveryReceipt:
        return cls(
            work_ref=str(data.get("work_ref", "")),
            stage=str(data.get("stage", "")),
            session_id=str(data.get("session_id", "")),
        )


@dataclass(frozen=True)
class ScopedDispatchResult:
    """Result of ``dispatch(..., expected_scope=)``."""

    receipt: DeliveryReceipt
    delivery_scope: DeliveryScope
    fencing_token: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ScopedDispatchResult:
        return cls(
            receipt=DeliveryReceipt.from_dict(data.get("receipt") or {}),
            delivery_scope=DeliveryScope.from_dict(data.get("delivery_scope") or {}),
            fencing_token=int(data.get("fencing_token", 0)),
        )


class ScopedRecoveryState(str, Enum):
    """What ``recover_delivery`` established, from the ORIGINAL session only."""

    #: An authoritative point-in-time miss. An outstanding admission can still
    #: land later: never permission to retry.
    ABSENT = "absent"
    #: The original session holds the input, not yet terminal.
    IN_FLIGHT = "in_flight"
    #: The delivery's turn completed (see ``output_status``).
    COMPLETED = "completed"
    #: The delivery's turn reached a failed terminal.
    FAILED = "failed"
    #: The input reached a terminal disposition without a run of its own.
    TERMINAL_WITHOUT_RUN = "terminal_without_run"
    #: Evidence exists but is internally inconsistent.
    BROKEN = "broken"
    #: The original owner could not establish the state (see ``cause``).
    UNRESOLVED = "unresolved"


@dataclass(frozen=True)
class ScopedRecovery:
    """Result of ``recover_delivery``. Only ``state`` is always set."""

    state: ScopedRecoveryState
    delivery_scope: DeliveryScope | None = None
    input_id: str | None = None
    #: ``in_flight``: the runtime input phase, and whether a committed store
    #: row backs it (``False`` for a live-only row).
    phase: Any = None
    durable_witness: bool | None = None
    #: ``completed``: as for :class:`TurnResult`.
    output_status: str | None = None
    output: str | None = None
    output_truncated: bool = False
    #: ``failed``.
    error: str | None = None
    #: ``terminal_without_run``.
    terminal: Any = None
    last_run_id: str | None = None
    #: ``broken``.
    reason: str | None = None
    #: ``unresolved``: ``runtime_adapter_unavailable``,
    #: ``original_session_unknown``, ``original_owner_unavailable``,
    #: ``evidence_read_timed_out`` or ``scope_binding_unproven``.
    cause: str | None = None
    detail: str | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ScopedRecovery:
        recovery = data.get("recovery") if isinstance(data.get("recovery"), dict) else data
        scope_raw = data.get("delivery_scope")
        raw_state = str(recovery.get("state", "unresolved"))
        try:
            state = ScopedRecoveryState(raw_state)
        except ValueError:
            # A state this SDK does not know is unknown, never absence.
            state = ScopedRecoveryState.UNRESOLVED
        return cls(
            state=state,
            delivery_scope=(
                DeliveryScope.from_dict(scope_raw) if isinstance(scope_raw, dict) else None
            ),
            input_id=recovery.get("input_id"),
            phase=recovery.get("phase"),
            durable_witness=recovery.get("durable_witness"),
            output_status=recovery.get("output_status"),
            output=recovery.get("output"),
            output_truncated=bool(recovery.get("output_truncated", False)),
            error=recovery.get("error"),
            terminal=recovery.get("terminal"),
            last_run_id=recovery.get("last_run_id"),
            reason=recovery.get("reason"),
            cause=(
                recovery.get("cause")
                if state is not ScopedRecoveryState.UNRESOLVED or raw_state == "unresolved"
                else f"unknown_state:{raw_state}"
            ),
            detail=recovery.get("detail"),
        )


class TurnState(str, Enum):
    """Where a ticketed turn stands (``meta/turn_result``)."""

    PENDING = "pending"
    COMPLETED = "completed"
    FAILED = "failed"
    #: No turn with this ticket is known for this identity.
    UNKNOWN = "unknown"


class TurnOutputStatus(str, Enum):
    """What a completed ticketed turn yielded (``output_status``)."""

    #: The turn's own final text is in ``output`` (see ``output_truncated``).
    TEXT = "text"
    #: The turn ran and committed a run result with no text.
    EMPTY = "empty"
    #: The admission completed without a run result of its own: the runtime
    #: folded it into a run already in progress, or deduplicated it onto an
    #: earlier admission whose turn had already ended. No output belongs to
    #: it.
    NO_OWN_RESULT = "no_own_result"
    #: The gateway's session bridge cannot report per-turn output (also used
    #: for a status this SDK does not know).
    UNAVAILABLE = "unavailable"


@dataclass(frozen=True)
class TurnResult:
    """``meta/turn_result``: one ticketed turn's state and its OWN output,
    from that turn's committed run result (never the session's latest text)."""

    identity: str
    ticket: str
    state: TurnState
    #: What a COMPLETED turn yielded; ``None`` in any other state.
    output_status: TurnOutputStatus | None = None
    #: The turn's own final text; set only when ``output_status`` is TEXT.
    output: str | None = None
    #: The text was cut at the gateway's bound (the runtime also appends a
    #: truncation marker to it).
    output_truncated: bool = False
    #: The failure detail when ``state`` is FAILED.
    error: str | None = None
    #: The identity-wide cursor at read time (diagnostic only).
    completion_cursor: CompletionCursor | None = None

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {
            "identity": self.identity,
            "ticket": self.ticket,
            "state": self.state.value,
        }
        if self.output_status is not None:
            result["output_status"] = self.output_status.value
            result["output"] = self.output
            result["output_truncated"] = self.output_truncated
        if self.error is not None:
            result["error"] = self.error
        if self.completion_cursor is not None:
            result["completion_cursor"] = self.completion_cursor.to_dict()
        return result

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> TurnResult:
        raw_state = data.get("state")
        try:
            state = TurnState(raw_state)
        except ValueError:
            state = TurnState.UNKNOWN
        output_status: TurnOutputStatus | None = None
        if state is TurnState.COMPLETED:
            try:
                output_status = TurnOutputStatus(data.get("output_status"))
            except ValueError:
                output_status = TurnOutputStatus.UNAVAILABLE
        output = data.get("output")
        error = data.get("error")
        return cls(
            identity=str(data.get("identity", "")),
            ticket=str(data.get("ticket", "")),
            state=state,
            output_status=output_status,
            output=(
                output
                if output_status is TurnOutputStatus.TEXT and isinstance(output, str)
                else None
            ),
            output_truncated=bool(data.get("output_truncated", False)),
            error=error if isinstance(error, str) else None,
            completion_cursor=_completion_cursor_from(data, "completion_cursor"),
        )


@dataclass(frozen=True)
class AwaitedTurn:
    """What a ``*_and_wait_outcome`` call observed after its delivery.

    ``attributed`` says whether ``text`` provably belongs to THIS delivery's
    turn. ``True``: the turn was tracked by ``ticket`` and ``text`` is its own
    committed output (``output_status`` says how; ``None`` text means it
    committed none). ``False``: the delivery could not be tracked
    (``untracked_code`` is the gateway's ``turn_unavailable.code``, ``None``
    for a gateway predating turn tickets), so the call waited on the
    identity-wide completion cursor and ``text`` is the session's latest
    output, which another delivery's turn (a peer message, a scheduled turn)
    may have produced. ``admission`` is always the send/dispatch result.
    """

    text: str | None
    attributed: bool
    admission: Any
    ticket: str | None = None
    output_status: TurnOutputStatus | None = None
    untracked_code: str | None = None


@dataclass(frozen=True)
class IdentityInspection:
    """Rich inspection of an identity's current execution state.

    ``agent_runtime_id``, ``session_id``, ``member_id``,
    :attr:`intent_managed`, :attr:`convergence`, :attr:`wake_refusal` and
    :attr:`replaced_session` read as on :class:`IdentityStatus`.
    """

    identity: str
    output_preview: str | None = None
    is_final: bool = False
    peer_reachable_count: int = 0
    #: Why ``output_preview`` is not an observation of the member's session
    #: (``"observation_deadline"``, ``"read_failed"``, ``"session_absent"``,
    #: ``"not_observed_while_retiring"``; unknown future values pass through).
    #: When set, a missing preview is not an empty member. ``None`` when the
    #: preview was observed, or the gateway predates the field.
    preview_unavailable: str | None = None
    #: Live completion cursor. ``None`` for non-identity-first live aliases
    #: (no identity authority tracks their turns) and for gateways predating
    #: the field — in both cases the caller must not read absence as "no turns
    #: yet".
    completion_cursor: CompletionCursor | None = None
    agent_runtime_id: str | None = None
    session_id: str | None = None
    intent_managed: bool = False
    convergence: IdentityConvergence | None = None
    wake_refusal: WakeRefusal | None = None
    replaced_session: ReplacedSession | None = None
    member_id: str | None = None

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {
            "identity": self.identity,
            "is_final": self.is_final,
            "peer_reachable_count": self.peer_reachable_count,
            "intent_managed": self.intent_managed,
        }
        if self.agent_runtime_id is not None:
            result["agent_runtime_id"] = self.agent_runtime_id
        if self.session_id is not None:
            result["session_id"] = self.session_id
        if self.member_id is not None:
            result["member_id"] = self.member_id
        if self.output_preview is not None:
            result["output_preview"] = self.output_preview
        if self.preview_unavailable is not None:
            result["preview_unavailable"] = self.preview_unavailable
        if self.completion_cursor is not None:
            result["completion_cursor"] = self.completion_cursor.to_dict()
        if self.convergence is not None:
            result["convergence"] = self.convergence.to_dict()
        if self.wake_refusal is not None:
            result["wake_refusal"] = self.wake_refusal.to_dict()
        if self.replaced_session is not None:
            result["replaced_session"] = self.replaced_session.to_dict()
        return result

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> IdentityInspection:
        unavailable = data.get("preview_unavailable")
        runtime_id = data.get("agent_runtime_id")
        session_id = data.get("session_id")
        member_id = data.get("member_id")
        return cls(
            identity=data["identity"],
            output_preview=data.get("output_preview"),
            preview_unavailable=unavailable if isinstance(unavailable, str) else None,
            is_final=bool(data.get("is_final", False)),
            peer_reachable_count=int(data.get("peer_reachable_count", 0)),
            completion_cursor=_completion_cursor_from(data, "completion_cursor"),
            agent_runtime_id=runtime_id if isinstance(runtime_id, str) else None,
            session_id=session_id if isinstance(session_id, str) else None,
            member_id=member_id if isinstance(member_id, str) else None,
            intent_managed=data.get("intent_managed") is True,
            convergence=_view_part(data, "convergence", IdentityConvergence),
            wake_refusal=_view_part(data, "wake_refusal", WakeRefusal),
            replaced_session=_view_part(data, "replaced_session", ReplacedSession),
        )


@dataclass(frozen=True)
class ConsoleIdentityRecord:
    """One roster member from ``meta/console/list_identities``.

    The typed identity -> session map: ``session_id`` is the member's
    current durable binding.

    ``labels`` are display data: ``labels["role"]`` keeps the roster
    provider's own value (often human role prose) and falls back to the
    profile name only when the provider set none. It is not profile or
    instruction authority; instructions come from the runtime's mob
    definition and the projected roster.
    """

    identity: str
    display_name: str
    runtime_key: str
    runtime_member_id: str
    visibility: str
    addressable: bool
    health: str
    session_id: str | None = None
    topology_peers: list[str] = field(default_factory=list)
    labels: dict[str, str] = field(default_factory=dict)
    #: Core's mob member id of the identity's last binding (Meta's
    #: comms-safe encoding of the identity), as on
    #: :attr:`IdentityStatus.member_id`; ``None`` when unknown, and for
    #: a raw runtime-alias record.
    member_id: str | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ConsoleIdentityRecord:
        return cls(
            identity=data["identity"],
            display_name=str(data.get("display_name", "")),
            runtime_key=str(data.get("runtime_key", "")),
            runtime_member_id=str(data.get("runtime_member_id", "")),
            visibility=str(data.get("visibility", "")),
            addressable=bool(data.get("addressable", False)),
            health=str(data.get("health", "")),
            session_id=data.get("session_id"),
            topology_peers=[str(peer) for peer in data.get("topology_peers", [])],
            labels={str(k): str(v) for k, v in (data.get("labels") or {}).items()},
            member_id=data.get("member_id"),
        )
