"""Typed data models for Meta SDK - matches the downstream app's import surface."""
from __future__ import annotations

from dataclasses import dataclass, field, fields
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from .jobs import DetachedJobExecution



_ARTIFACT_PART_MAX_BYTES = 256


@dataclass(frozen=True)
class ToolArtifactRef:
    """An installed artifact a callback tool's handler came from.

    An opaque artifact id and the digest the host installed. The gateway
    verifies it against the host's ``MetaBuilder.installed_artifacts(...)``
    before the build publishes any tool, and carries a verified reference on
    the tool's provenance. Attribution only: it never grants or refuses a
    call, and a tool's name never implies one. It is not proof of the
    executable bytes or the semantics of the handler that runs. The host's
    installed set is trusted configuration from its own bootstrap.
    """

    artifact_id: str
    digest: str

    def __post_init__(self) -> None:
        for label, part in (("artifact_id", self.artifact_id), ("digest", self.digest)):
            if not isinstance(part, str) or not part:
                raise ValueError(f"tool artifact {label} must be a non-empty string")
            if len(part.encode("utf-8")) > _ARTIFACT_PART_MAX_BYTES:
                raise ValueError(
                    f"tool artifact {label} exceeds {_ARTIFACT_PART_MAX_BYTES} bytes"
                )
            if any(ch.isspace() or not ch.isprintable() for ch in part):
                raise ValueError(
                    f"tool artifact {label} must not contain whitespace or control characters"
                )

    def to_wire(self) -> dict[str, str]:
        return {"artifact_id": self.artifact_id, "digest": self.digest}

    @classmethod
    def from_wire(cls, data: dict[str, Any]) -> ToolArtifactRef:
        return cls(artifact_id=data["artifact_id"], digest=data["digest"])


@dataclass
class DiscoverySpec:
    """Agent discovery specification.

    Maps to Rust SpawnMemberSpec fields via the Meta discovery pipeline.
    """

    role: str
    agent_identity: str
    labels: dict[str, str] = field(default_factory=dict)
    app_context: Any | None = None
    additional_instructions: list[str] = field(default_factory=list)
    resume_session_id: str | None = None

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {
            "role": self.role,
            "agent_identity": self.agent_identity,
        }
        if self.labels:
            result["labels"] = dict(self.labels)
        if self.app_context is not None:
            result["app_context"] = self.app_context
        if self.additional_instructions:
            result["additional_instructions"] = list(self.additional_instructions)
        if self.resume_session_id is not None:
            result["resume_session_id"] = self.resume_session_id
        return result


@dataclass
class PreSpawnData:
    """Pre-spawn data for session resume and cache warming.

    The resume_map maps agent_identity -> session_id for agents that should
    resume existing sessions instead of creating new ones.
    """

    resume_map: dict[str, str] = field(default_factory=dict)
    module_id: str | None = None
    env: dict[str, str] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {}
        if self.resume_map:
            result["resume_map"] = dict(self.resume_map)
        if self.module_id is not None:
            result["module_id"] = self.module_id
        if self.env:
            result["env"] = list(self.env.items())
        return result


@dataclass
class SessionQuery:
    """Query parameters for session lookup."""

    agent_type: str | None = None
    owner_id: str | None = None
    labels: dict[str, str] = field(default_factory=dict)
    include_deleted: bool = False
    limit: int = 100

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {}
        if self.agent_type is not None:
            result["agent_type"] = self.agent_type
        if self.owner_id is not None:
            result["owner_id"] = self.owner_id
        if self.labels:
            result["labels"] = dict(self.labels)
        result["include_deleted"] = self.include_deleted
        result["limit"] = self.limit
        return result


@dataclass(frozen=True)
class SessionCreatedContext:
    """Context delivered to SessionAgentBuilder.after_create after a session
    is successfully created."""

    model: str
    labels: dict[str, str]
    system_prompt: str | None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> SessionCreatedContext:
        return cls(
            model=data.get("model", ""),
            labels=dict(data.get("labels") or {}),
            system_prompt=data.get("system_prompt"),
        )


def _required_str(data: dict[str, Any], key: str, owner: str) -> str:
    value = data.get(key)
    if not isinstance(value, str):
        raise TypeError(f"{owner}.{key} must be a string, got {type(value).__name__}: {value!r}")
    return value


@dataclass(frozen=True)
class MobMemberBinding:
    """A mob member as meerkat names it: its mob, role and roster member id.

    In a Meta mob ``member`` is Meta's comms-safe roster encoding of the
    member's durable identity (``mk--...``), not the identity itself.
    """

    mob_id: str
    role: str
    member: str

    @classmethod
    def from_dict(cls, data: Any) -> MobMemberBinding:
        """Decode the wire object. Fields this SDK does not know are ignored."""
        if not isinstance(data, dict):
            raise TypeError(f"MobMemberBinding must be an object, got {type(data).__name__}")
        return cls(
            mob_id=_required_str(data, "mob_id", "MobMemberBinding"),
            role=_required_str(data, "role", "MobMemberBinding"),
            member=_required_str(data, "member", "MobMemberBinding"),
        )

    def to_dict(self) -> dict[str, Any]:
        return {"mob_id": self.mob_id, "role": self.role, "member": self.member}


@dataclass(frozen=True)
class ForkBuildSource:
    """The source a fork-derived member was forked from (meerkat 0.8.45+).

    Set by the mob runtime on the build that seats a durable fork (fork_off
    children, fork_member children, local council participants) and on every
    later rebuild of that member. Absent for every other build.
    """

    source_member: MobMemberBinding
    source_session_id: str

    @classmethod
    def from_dict(cls, data: Any) -> ForkBuildSource:
        """Decode the wire object. Fields this SDK does not know are ignored,
        so a newer gateway can add fields without breaking older builders."""
        if not isinstance(data, dict):
            raise TypeError(f"ForkBuildSource must be an object, got {type(data).__name__}")
        return cls(
            source_member=MobMemberBinding.from_dict(data.get("source_member")),
            source_session_id=_required_str(data, "source_session_id", "ForkBuildSource"),
        )

    def to_dict(self) -> dict[str, Any]:
        return {
            "source_member": self.source_member.to_dict(),
            "source_session_id": self.source_session_id,
        }


@dataclass
class SessionBuildOptions:
    """Options passed to SessionAgentBuilder.build_agent().

    Mutable — the builder mutates fields during agent construction.

    ``fork_source`` and ``fork_source_identity`` are receive-only: they tell
    the builder that this member is a fork and which member it was forked
    from, so it can build the child as its source. They are never sent back,
    and the gateway ignores them in a build response: fork lineage is set by
    the mob runtime alone. ``fork_source.source_member.member`` is Meta's
    encoded roster id; ``fork_source_identity`` is the source's durable
    identity (for example ``domain:calendar``), the key to resolve grants by.
    The child's own ``labels`` and ``session_id`` still name the child.
    """

    app_context: Any | None = None
    additional_instructions: list[str] = field(default_factory=list)
    session_id: str | None = None
    labels: dict[str, str] = field(default_factory=dict)
    profile_name: str | None = None
    resume_session_id: str | None = None
    fork_source: ForkBuildSource | None = None
    fork_source_identity: str | None = None
    _tools: list[str] = field(default_factory=list, repr=False)
    _tool_handlers: dict[str, Any] = field(default_factory=dict, repr=False)
    # Per-tool wire metadata from register_tool(description=/input_schema=);
    # tools without an entry cross the wire as bare name strings.
    _tool_defs: dict[str, dict[str, Any]] = field(default_factory=dict, repr=False)
    # Host-only runner implementations paired with their private execution
    # declarations. Only the declarations cross the build callback wire.
    _job_executions: dict[str, DetachedJobExecution] = field(
        default_factory=dict,
        repr=False,
    )

    @classmethod
    def from_callback_options(cls, raw: dict[str, Any]) -> SessionBuildOptions:
        """Decode the gateway's ``callback/build_agent`` options.

        Keys that are not SessionBuildOptions fields are informational (the
        gateway also sends model and prompt) or come from a newer gateway, and
        are ignored. ``fork_source`` is decoded to a typed ForkBuildSource.
        """
        known = {f.name for f in fields(cls)}
        filtered = {k: v for k, v in raw.items() if k in known}
        fork_source = filtered.pop("fork_source", None)
        fork_source_identity = filtered.pop("fork_source_identity", None)
        opts = cls(**filtered)
        if fork_source is not None:
            opts.fork_source = ForkBuildSource.from_dict(fork_source)
        if fork_source_identity is not None:
            if not isinstance(fork_source_identity, str):
                raise TypeError(
                    "fork_source_identity must be a string, got "
                    f"{type(fork_source_identity).__name__}: {fork_source_identity!r}"
                )
            opts.fork_source_identity = fork_source_identity
        return opts

    def add_tools(self, tools: list[str]) -> None:
        """Declare tool names the agent can use."""
        for t in tools:
            if not isinstance(t, str):
                raise TypeError(f"tools must be strings, got {type(t).__name__}: {t!r}")
        self._tools.extend(tools)

    def register_tool(
        self,
        name: str,
        handler: Any,
        *,
        description: str = "",
        input_schema: dict[str, Any] | None = None,
        execution: DetachedJobExecution | None = None,
        artifact: ToolArtifactRef | None = None,
    ) -> None:
        """Register a callable tool with the agent.

        The handler is called when the agent invokes this tool. It receives
        a dict of arguments and should return a JSON-serializable result.

        Args:
            name: Tool name (string).
            handler: Async or sync callable ``(args: dict) -> Any``.
            description: Human-readable tool description.
            input_schema: JSON Schema for the tool arguments. When omitted
                the gateway advertises the permissive ``{"type": "object"}``.
            execution: Optional detached-job declaration and host runner.
            artifact: Optional installed artifact this handler came from,
                verified by the gateway against the host's installed artifacts.
        """
        if not isinstance(name, str):
            raise TypeError(f"tool name must be a string, got {type(name).__name__}: {name!r}")
        if not callable(handler):
            raise TypeError(f"handler must be callable, got {type(handler).__name__}: {handler!r}")
        if input_schema is not None and not isinstance(input_schema, dict):
            raise TypeError(
                f"input_schema must be a dict, got {type(input_schema).__name__}: {input_schema!r}"
            )
        if execution is not None:
            from .jobs import DetachedJobExecution

            if not isinstance(execution, DetachedJobExecution):
                raise TypeError(
                    "execution must be DetachedJobExecution, got "
                    f"{type(execution).__name__}: {execution!r}"
                )
        if artifact is not None and not isinstance(artifact, ToolArtifactRef):
            raise TypeError(
                f"artifact must be ToolArtifactRef, got {type(artifact).__name__}: {artifact!r}"
            )
        # Registering a name again replaces its handler AND all of its
        # metadata: a registration without an artifact never inherits the
        # previous handler's artifact (or description, schema, execution).
        if name not in self._tools:
            self._tools.append(name)
        self._tool_defs.pop(name, None)
        self._job_executions.pop(name, None)
        self._tool_handlers[name] = handler
        if (
            description
            or input_schema is not None
            or execution is not None
            or artifact is not None
        ):
            tool_def: dict[str, Any] = {"name": name}
            if description:
                tool_def["description"] = description
            if input_schema is not None:
                tool_def["input_schema"] = input_schema
            if execution is not None:
                tool_def["execution"] = execution.to_wire()
                self._job_executions[name] = execution
            if artifact is not None:
                tool_def["artifact"] = artifact.to_wire()
            self._tool_defs[name] = tool_def

    @property
    def tools(self) -> list[str]:
        return list(self._tools)

    @property
    def tool_handlers(self) -> dict[str, Any]:
        return dict(self._tool_handlers)

    @property
    def job_executions(self) -> dict[str, DetachedJobExecution]:
        return dict(self._job_executions)

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {}
        if self.app_context is not None:
            result["app_context"] = self.app_context
        if self.additional_instructions:
            result["additional_instructions"] = list(self.additional_instructions)
        if self.session_id is not None:
            result["session_id"] = self.session_id
        if self.labels:
            result["labels"] = dict(self.labels)
        if self.profile_name is not None:
            result["profile_name"] = self.profile_name
        if self.resume_session_id is not None:
            result["resume_session_id"] = self.resume_session_id
        if self._tools:
            # Names with registered metadata cross as {name, description?,
            # input_schema?} objects; everything else stays a bare string
            # (backward-compatible with pre-0.7.30 gateways).
            result["tools"] = [
                self._tool_defs.get(name, name) for name in self._tools
            ]
        return result
