"""Versioned Meta live-channel wire contracts."""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Awaitable, Callable, Literal, Protocol

LIVE_EXECUTION_IDENTITY_V1 = "live.execution_identity.v1"
LIVE_EXECUTION_FUNCTION_BRIDGE_V1 = "live.execution.function_bridge.v1"
LIVE_EXECUTION_CLIENT_CONTEXT_V1 = "live.execution.client_context.v1"

LiveExecutionMode = Literal["function_bridge", "client_context"]
_EXECUTION_MODE_CAPABILITIES: dict[str, str] = {
    "function_bridge": LIVE_EXECUTION_FUNCTION_BRIDGE_V1,
    "client_context": LIVE_EXECUTION_CLIENT_CONTEXT_V1,
}

# Public Live (``gpt-live-1``) client-context execution profile served by a
# gateway registered through ``runtime_options.openai_live``. Callers select
# it through ``live/open { execution_identity: { version: "v1", profile_id } }``.
OPENAI_GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID = "openai.gpt-live-1.client-context.v1"

_RESERVED_GPT_LIVE_PROFILE_IDS = {
    "openai.gpt-live-1-codex.client-context.v1",
    "openai.gpt-live-1-codex.function-bridge.v1",
    OPENAI_GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID,
}


def _exact(data: dict[str, Any], allowed: set[str], context: str) -> None:
    unknown = set(data) - allowed
    if unknown:
        raise ValueError(f"{context} contains unknown field {sorted(unknown)[0]}")


def _string(data: dict[str, Any], field: str, context: str) -> str:
    value = data.get(field)
    return _non_empty(value, f"{context}.{field}")


def _non_empty(value: Any, context: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{context} must be a non-empty string")
    return value


def _boolean(data: dict[str, Any], field: str, context: str) -> bool:
    value = data.get(field)
    if not isinstance(value, bool):
        raise ValueError(f"{context}.{field} must be a boolean")
    return value


def _advisories(data: dict[str, Any], context: str) -> tuple[str, ...]:
    """Return what a successful open went ahead without.

    The server omits the field when there is nothing to report, read as
    ``()``. ``"staged_prompt_refused"`` means a run-start hook refused the
    session's staged create-time prompt, which stays staged for its first
    text turn. The vocabulary may grow: an advisory this SDK does not know is
    kept as the string the server sent, so a newer server never fails the
    open. A field that is not a list of non-empty strings fails closed.
    """
    raw = data.get("advisories", [])
    if not isinstance(raw, list):
        raise ValueError(f"{context}.advisories must be a list")
    return tuple(_non_empty(value, f"{context}.advisories") for value in raw)


@dataclass(frozen=True)
class LiveAuthBindingRef:
    realm: str
    binding: str
    profile: str | None = None

    def to_dict(self) -> dict[str, str]:
        if not isinstance(self.realm, str) or not self.realm.strip():
            raise ValueError("auth binding.realm must be a non-empty string")
        if not isinstance(self.binding, str) or not self.binding.strip():
            raise ValueError("auth binding.binding must be a non-empty string")
        if self.profile is not None and (
            not isinstance(self.profile, str) or not self.profile.strip()
        ):
            raise ValueError("auth binding.profile must be a non-empty string")
        result = {"realm": self.realm, "binding": self.binding}
        if self.profile is not None:
            result["profile"] = self.profile
        return result

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> LiveAuthBindingRef:
        _exact(data, {"realm", "binding", "profile"}, "auth binding")
        profile = data.get("profile")
        if "profile" in data and (
            not isinstance(profile, str) or not profile.strip()
        ):
            raise ValueError("auth binding.profile must be a non-empty string")
        return cls(
            realm=_string(data, "realm", "auth binding"),
            binding=_string(data, "binding", "auth binding"),
            profile=profile,
        )


def _positive_int(value: Any, context: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise ValueError(f"{context} must be a positive integer")
    return value


@dataclass(frozen=True)
class OpenAiLiveSummaryConfig:
    """Bounds for the console voice context summary (``console_voice.summary``).

    ``max_input_bytes`` is the most recent slice of the serialized transcript
    handed to the summariser (gateway default 65536), ``max_output_bytes`` the
    UTF-8 cap on the produced summary (gateway default 4096), and ``model`` the
    summary model on the agent's provider and credentials (default: the agent's
    own model). Every field is optional; unset fields keep the gateway default.
    """

    model: str | None = None
    max_input_bytes: int | None = None
    max_output_bytes: int | None = None

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {}
        if self.model is not None:
            result["model"] = _non_empty(self.model, "openai live summary.model")
        if self.max_input_bytes is not None:
            result["max_input_bytes"] = _positive_int(
                self.max_input_bytes, "openai live summary.max_input_bytes"
            )
        if self.max_output_bytes is not None:
            result["max_output_bytes"] = _positive_int(
                self.max_output_bytes, "openai live summary.max_output_bytes"
            )
        return result


@dataclass(frozen=True)
class OpenAiLiveGatewayConfig:
    """Public OpenAI Live (``gpt-live-1``) host registration.

    Serialized as ``runtime_options.openai_live`` by ``openai_live()`` or
    ``runtime_options.console_voice`` by ``console_voice()``. The gateway fixes the
    execution identity (provider OpenAI, model ``gpt-live-1``, this configured
    API-key binding); the host names only the principal, realm, binding,
    voice, and optional trusted session instructions. Requires a gateway
    compiled with ``openai-live``. The stdio registration is mutually exclusive
    with the deprecated :class:`ExperimentalLiveGatewayConfig`.
    """

    principal: str
    realm: str
    auth_binding: LiveAuthBindingRef
    voice: str
    session_instructions: str | None = None
    summary: OpenAiLiveSummaryConfig | None = None

    def to_dict(self) -> dict[str, Any]:
        values = {
            "principal": self.principal,
            "realm": self.realm,
            "voice": self.voice,
        }
        for name, value in values.items():
            if not isinstance(value, str) or not value.strip():
                raise ValueError(f"openai live {name} must be a non-empty string")
        if not isinstance(self.auth_binding, LiveAuthBindingRef):
            raise TypeError("openai live auth_binding must be a LiveAuthBindingRef")
        if self.auth_binding.realm != self.realm:
            raise ValueError("openai live auth binding realm must equal realm")
        result: dict[str, Any] = {
            **values,
            "auth_binding": self.auth_binding.to_dict(),
        }
        if self.session_instructions is not None:
            result["session_instructions"] = _non_empty(
                self.session_instructions, "openai live session_instructions"
            )
        if self.summary is not None:
            if not isinstance(self.summary, OpenAiLiveSummaryConfig):
                raise TypeError("openai live summary must be an OpenAiLiveSummaryConfig")
            result["summary"] = self.summary.to_dict()
        return result


@dataclass(frozen=True)
class ExperimentalLiveExecutionProfileConfig:
    """One host-trusted, caller-selectable live instruction profile."""

    profile_id: str
    session_instructions: str

    def to_dict(self) -> dict[str, str]:
        profile_id = _non_empty(
            self.profile_id, "experimental live execution profile.profile_id"
        ).strip()
        if profile_id in _RESERVED_GPT_LIVE_PROFILE_IDS:
            raise ValueError("experimental live execution profile.profile_id is reserved")
        return {
            "profile_id": profile_id,
            "session_instructions": _non_empty(
                self.session_instructions,
                "experimental live execution profile.session_instructions",
            ).strip(),
        }


@dataclass(frozen=True)
class ExperimentalLiveGatewayConfig:
    """DEPRECATED explicit host registration for the pre-release GPT Live lane.

    Prefer :class:`OpenAiLiveGatewayConfig` (``runtime_options.openai_live``).

    No field defaults an authority decision. The gateway remains disabled
    when this object is absent, and a build without matching Gate0 evidence
    still advertises no experimental capability.
    """

    principal: str
    realm: str
    factory_kind: str
    factory_version: str
    gate0_qualification: str
    auth_binding: LiveAuthBindingRef
    voice: str
    execution_profiles: tuple[ExperimentalLiveExecutionProfileConfig, ...] = ()

    def to_dict(self) -> dict[str, Any]:
        values = {
            "principal": self.principal,
            "realm": self.realm,
            "factory_kind": self.factory_kind,
            "factory_version": self.factory_version,
            "gate0_qualification": self.gate0_qualification,
            "voice": self.voice,
        }
        for name, value in values.items():
            if not isinstance(value, str) or not value.strip():
                raise ValueError(f"experimental live {name} must be a non-empty string")
        if self.auth_binding.realm != self.realm:
            raise ValueError("experimental live auth binding realm must equal realm")
        result: dict[str, Any] = {
            **values,
            "auth_binding": self.auth_binding.to_dict(),
        }
        if not isinstance(self.execution_profiles, tuple):
            raise TypeError("experimental live execution_profiles must be a tuple")
        seen: set[str] = set()
        execution_profiles: list[dict[str, str]] = []
        for profile in self.execution_profiles:
            if not isinstance(profile, ExperimentalLiveExecutionProfileConfig):
                raise TypeError(
                    "experimental live execution_profiles entries must be "
                    "ExperimentalLiveExecutionProfileConfig"
                )
            serialized = profile.to_dict()
            profile_id = serialized["profile_id"]
            if profile_id in seen:
                raise ValueError(
                    f"experimental live execution_profiles contains duplicate "
                    f"profile_id {profile_id}"
                )
            seen.add(profile_id)
            execution_profiles.append(serialized)
        if execution_profiles:
            result["execution_profiles"] = execution_profiles
        return result


@dataclass(frozen=True)
class LiveExecutionIdentityV1:
    version: Literal["v1"] = field(default="v1", init=False)
    profile_id: str

    def to_dict(self) -> dict[str, Any]:
        return {
            "version": self.version,
            "profile_id": _non_empty(self.profile_id, "profile_id"),
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> LiveExecutionIdentityV1:
        _exact(data, {"version", "profile_id"}, "execution identity")
        if data.get("version") != "v1":
            raise ValueError("execution identity.version must be v1")
        return cls(
            profile_id=_string(data, "profile_id", "execution identity"),
        )


def live_open_execution_identity_params(
    execution_identity: LiveExecutionIdentityV1,
    **legacy: Any,
) -> dict[str, Any]:
    """Serialize the v1 envelope and reject ambiguous legacy mixing."""
    for field in [
        "model",
        "provider",
        "auth_binding",
        "self_hosted_server_id",
        "provider_params",
    ]:
        if field in legacy:
            raise ValueError(
                f"execution_identity conflicts with legacy top-level {field}"
            )
    return {"execution_identity": execution_identity.to_dict()}


def supports_live_execution_identity_v1(feature_capabilities: list[str]) -> bool:
    return LIVE_EXECUTION_IDENTITY_V1 in feature_capabilities


def live_execution_mode_capability(mode: LiveExecutionMode) -> str:
    try:
        return _EXECUTION_MODE_CAPABILITIES[mode]
    except KeyError as error:
        raise ValueError("unknown provider-neutral live execution mode") from error


def supports_live_execution_mode(
    feature_capabilities: list[str], mode: LiveExecutionMode
) -> bool:
    return live_execution_mode_capability(mode) in feature_capabilities


@dataclass(frozen=True)
class LiveTransportBootstrap:
    transport: Literal["websocket", "webrtc", "unknown"]
    token: str | None = None
    url: str | None = None
    answer_method: str | None = None
    http_url: str | None = None
    debug: str | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> LiveTransportBootstrap:
        transport = _string(data, "transport", "transport")
        if transport == "websocket":
            _exact(data, {"transport", "url", "token"}, "websocket transport")
            return cls(
                transport="websocket",
                url=_string(data, "url", "transport"),
                token=_string(data, "token", "transport"),
            )
        if transport == "webrtc":
            _exact(
                data,
                {"transport", "token", "answer_method", "http_url"},
                "webrtc transport",
            )
            http_url = data.get("http_url")
            if http_url is not None and (not isinstance(http_url, str) or not http_url):
                raise ValueError("transport.http_url must be a non-empty string")
            return cls(
                transport="webrtc",
                token=_string(data, "token", "transport"),
                answer_method=_string(data, "answer_method", "transport"),
                http_url=http_url,
            )
        if transport == "unknown":
            _exact(data, {"transport", "debug"}, "unknown transport")
            return cls(transport="unknown", debug=_string(data, "debug", "transport"))
        raise ValueError(f"unknown live transport {transport}")

    def to_dict(self) -> dict[str, Any]:
        if self.transport == "websocket" and self.url and self.token:
            return {"transport": "websocket", "url": self.url, "token": self.token}
        if self.transport == "webrtc" and self.token and self.answer_method:
            result = {
                "transport": "webrtc",
                "token": self.token,
                "answer_method": self.answer_method,
            }
            if self.http_url is not None:
                result["http_url"] = self.http_url
            return result
        if self.transport == "unknown" and self.debug:
            return {"transport": "unknown", "debug": self.debug}
        raise ValueError("incomplete live transport bootstrap")


@dataclass(frozen=True)
class LiveChannelCapabilities:
    audio_in: bool
    audio_out: bool
    text_in: bool
    text_out: bool
    image_in: bool
    video_in: bool
    transcript_supported: bool
    barge_in_supported: bool
    provider_native_resume: bool

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> LiveChannelCapabilities:
        names = {
            "audio_in", "audio_out", "text_in", "text_out", "image_in", "video_in",
            "transcript_supported", "barge_in_supported", "provider_native_resume",
        }
        _exact(data, names, "capabilities")
        return cls(**{name: _boolean(data, name, "capabilities") for name in names})

    def to_dict(self) -> dict[str, bool]:
        return {
            "audio_in": self.audio_in,
            "audio_out": self.audio_out,
            "text_in": self.text_in,
            "text_out": self.text_out,
            "image_in": self.image_in,
            "video_in": self.video_in,
            "transcript_supported": self.transcript_supported,
            "barge_in_supported": self.barge_in_supported,
            "provider_native_resume": self.provider_native_resume,
        }


@dataclass(frozen=True)
class LiveContinuityMode:
    mode: Literal[
        "fresh", "transcript_only", "degraded", "provider_native_resume", "unknown"
    ]
    provider_session_id: str | None = None
    debug: str | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> LiveContinuityMode:
        mode = _string(data, "mode", "continuity")
        if mode in {"fresh", "transcript_only", "degraded"}:
            _exact(data, {"mode"}, "continuity")
            return cls(mode=mode)
        if mode == "provider_native_resume":
            _exact(data, {"mode", "provider_session_id"}, "continuity")
            return cls(
                mode="provider_native_resume",
                provider_session_id=_string(data, "provider_session_id", "continuity"),
            )
        if mode == "unknown":
            _exact(data, {"mode", "debug"}, "continuity")
            return cls(mode="unknown", debug=_string(data, "debug", "continuity"))
        raise ValueError(f"unknown live continuity mode {mode}")

    def to_dict(self) -> dict[str, str]:
        if self.mode in {"fresh", "transcript_only", "degraded"}:
            return {"mode": self.mode}
        if self.mode == "provider_native_resume" and self.provider_session_id:
            return {"mode": self.mode, "provider_session_id": self.provider_session_id}
        if self.mode == "unknown" and self.debug:
            return {"mode": self.mode, "debug": self.debug}
        raise ValueError("incomplete live continuity mode")


@dataclass(frozen=True)
class LiveChannelHandle:
    channel_id: str
    target_identity: str
    transport: LiveTransportBootstrap
    capabilities: LiveChannelCapabilities
    continuity: LiveContinuityMode
    # What the open went ahead without, such as ``"staged_prompt_refused"``;
    # empty when the server reported nothing. Unknown values are kept as sent.
    advisories: tuple[str, ...] = ()

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> LiveChannelHandle:
        _exact(
            data,
            {
                "channel_id",
                "target_identity",
                "transport",
                "capabilities",
                "continuity",
                "advisories",
            },
            "live channel handle",
        )
        transport = data.get("transport")
        capabilities = data.get("capabilities")
        continuity = data.get("continuity")
        if not all(isinstance(value, dict) for value in (transport, capabilities, continuity)):
            raise ValueError("live channel handle nested fields must be objects")
        return cls(
            channel_id=_string(data, "channel_id", "live channel handle"),
            target_identity=_string(data, "target_identity", "live channel handle"),
            transport=LiveTransportBootstrap.from_dict(transport),
            capabilities=LiveChannelCapabilities.from_dict(capabilities),
            continuity=LiveContinuityMode.from_dict(continuity),
            advisories=_advisories(data, "live channel handle"),
        )

    def to_dict(self) -> dict[str, Any]:
        wire: dict[str, Any] = {
            "channel_id": self.channel_id,
            "target_identity": self.target_identity,
            "transport": self.transport.to_dict(),
            "capabilities": self.capabilities.to_dict(),
            "continuity": self.continuity.to_dict(),
        }
        if self.advisories:
            wire["advisories"] = list(self.advisories)
        return wire


def _execution_mode(data: dict[str, Any], context: str) -> LiveExecutionMode:
    mode = _string(data, "execution_mode", context)
    if mode not in _EXECUTION_MODE_CAPABILITIES:
        raise ValueError(f"{context}.execution_mode is unknown")
    return mode  # type: ignore[return-value]


@dataclass(frozen=True)
class PendingLiveChannelHandle:
    channel_id: str
    target_identity: str
    execution_mode: LiveExecutionMode
    pending_receipt: str
    transport: LiveTransportBootstrap
    capabilities: LiveChannelCapabilities
    continuity: LiveContinuityMode
    # What the open went ahead without, as on ``LiveChannelHandle``.
    advisories: tuple[str, ...] = ()

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> PendingLiveChannelHandle:
        _exact(
            data,
            {
                "channel_id",
                "target_identity",
                "execution_mode",
                "pending_receipt",
                "transport",
                "capabilities",
                "continuity",
                "advisories",
            },
            "pending live channel handle",
        )
        transport = data.get("transport")
        capabilities = data.get("capabilities")
        continuity = data.get("continuity")
        if not all(isinstance(value, dict) for value in (transport, capabilities, continuity)):
            raise ValueError("pending live channel handle nested fields must be objects")
        return cls(
            channel_id=_string(data, "channel_id", "pending live channel handle"),
            target_identity=_string(data, "target_identity", "pending live channel handle"),
            execution_mode=_execution_mode(data, "pending live channel handle"),
            pending_receipt=_string(data, "pending_receipt", "pending live channel handle"),
            transport=LiveTransportBootstrap.from_dict(transport),
            capabilities=LiveChannelCapabilities.from_dict(capabilities),
            continuity=LiveContinuityMode.from_dict(continuity),
            advisories=_advisories(data, "pending live channel handle"),
        )

    def to_dict(self) -> dict[str, Any]:
        live_execution_mode_capability(self.execution_mode)
        wire: dict[str, Any] = {
            "channel_id": _non_empty(self.channel_id, "pending channel_id"),
            "target_identity": _non_empty(
                self.target_identity, "pending target_identity"
            ),
            "execution_mode": self.execution_mode,
            "pending_receipt": _non_empty(
                self.pending_receipt, "pending receipt"
            ),
            "transport": self.transport.to_dict(),
            "capabilities": self.capabilities.to_dict(),
            "continuity": self.continuity.to_dict(),
        }
        if self.advisories:
            wire["advisories"] = list(self.advisories)
        return wire


@dataclass(frozen=True)
class ActiveLiveChannelHandle:
    channel_id: str
    target_identity: str
    execution_mode: LiveExecutionMode
    activation_receipt: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ActiveLiveChannelHandle:
        _exact(
            data,
            {"channel_id", "target_identity", "execution_mode", "activation_receipt"},
            "active live channel handle",
        )
        return cls(
            channel_id=_string(data, "channel_id", "active live channel handle"),
            target_identity=_string(data, "target_identity", "active live channel handle"),
            execution_mode=_execution_mode(data, "active live channel handle"),
            activation_receipt=_string(
                data, "activation_receipt", "active live channel handle"
            ),
        )

    def to_dict(self) -> dict[str, str]:
        live_execution_mode_capability(self.execution_mode)
        return {
            "channel_id": _non_empty(self.channel_id, "active channel_id"),
            "target_identity": _non_empty(
                self.target_identity, "active target_identity"
            ),
            "execution_mode": self.execution_mode,
            "activation_receipt": _non_empty(
                self.activation_receipt, "activation receipt"
            ),
        }


@dataclass(frozen=True)
class LivePlaybackOwnerReadiness:
    channel_id: str
    readiness_receipt: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> LivePlaybackOwnerReadiness:
        _exact(data, {"channel_id", "readiness_receipt"}, "playback owner readiness")
        return cls(
            channel_id=_string(data, "channel_id", "playback owner readiness"),
            readiness_receipt=_string(
                data, "readiness_receipt", "playback owner readiness"
            ),
        )

    def to_dict(self) -> dict[str, str]:
        return {
            "channel_id": _non_empty(self.channel_id, "readiness channel_id"),
            "readiness_receipt": _non_empty(
                self.readiness_receipt, "readiness receipt"
            ),
        }


@dataclass(frozen=True)
class ExperimentalLiveChannelStatus:
    phase: Literal["pending", "active", "revoked", "closed"]
    handle: ActiveLiveChannelHandle | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ExperimentalLiveChannelStatus:
        phase = _string(data, "phase", "experimental live channel status")
        if phase in {"pending", "revoked", "closed"}:
            _exact(data, {"phase"}, "experimental live channel status")
            return cls(phase=phase)  # type: ignore[arg-type]
        if phase == "active":
            _exact(data, {"phase", "handle"}, "experimental live channel status")
            handle = data.get("handle")
            if not isinstance(handle, dict):
                raise ValueError("active live channel status.handle must be an object")
            return cls(phase="active", handle=ActiveLiveChannelHandle.from_dict(handle))
        raise ValueError("experimental live channel status.phase is unknown")

    def to_dict(self) -> dict[str, Any]:
        if self.phase in {"pending", "revoked", "closed"} and self.handle is None:
            return {"phase": self.phase}
        if self.phase == "active" and self.handle is not None:
            return {"phase": "active", "handle": self.handle.to_dict()}
        raise ValueError("experimental live channel status is incomplete")


class ActiveLiveChannelConnection(ActiveLiveChannelHandle):
    """Active handle plus exact playback-owner revocation custody.

    Use this as an async context manager or call :meth:`owner_lost` when the
    local media transport disappears. Both paths revoke active provider
    authority without retiring the durable member.
    """

    def __init__(
        self,
        active: ActiveLiveChannelHandle,
        pending_receipt: str,
        readiness_receipt: str,
        owner_lost: Callable[[], Awaitable[ExperimentalLiveChannelStatus]],
    ) -> None:
        super().__init__(
            channel_id=active.channel_id,
            target_identity=active.target_identity,
            execution_mode=active.execution_mode,
            activation_receipt=active.activation_receipt,
        )
        object.__setattr__(
            self, "pending_receipt", _non_empty(pending_receipt, "pending receipt")
        )
        object.__setattr__(
            self,
            "readiness_receipt",
            _non_empty(readiness_receipt, "readiness receipt"),
        )
        object.__setattr__(self, "_owner_lost", owner_lost)

    pending_receipt: str
    readiness_receipt: str
    _owner_lost: Callable[[], Awaitable[ExperimentalLiveChannelStatus]]

    @property
    def active_handle(self) -> ActiveLiveChannelHandle:
        return ActiveLiveChannelHandle(
            channel_id=self.channel_id,
            target_identity=self.target_identity,
            execution_mode=self.execution_mode,
            activation_receipt=self.activation_receipt,
        )

    async def owner_lost(self) -> ExperimentalLiveChannelStatus:
        return await self._owner_lost()

    async def __aenter__(self) -> ActiveLiveChannelConnection:
        return self

    async def __aexit__(self, _exc_type: Any, _exc: Any, _tb: Any) -> None:
        await self.owner_lost()


class LivePlaybackOwner(Protocol):
    """Local media owner used by the high-level experimental connector.

    ``prepare`` must install the bounded output consumer and gate microphone
    plus remote audio before returning an SDP offer. ``activate`` is the only
    method permitted to release those gates.
    """

    async def prepare(self, pending: PendingLiveChannelHandle) -> str: ...

    async def accept_answer(self, answer_sdp: str) -> None: ...

    async def activate(self, active: ActiveLiveChannelHandle) -> None: ...

    async def abort(self) -> None: ...

    # Implementations may additionally expose `wait_for_loss()`. The runtime
    # detects it dynamically and revokes machine authority when it completes
    # or fails.


@dataclass(frozen=True)
class LiveReplacementRequired:
    required: bool
    reason: Literal["canonical_context", "delegation_result"] | None = None
    replacement: LiveChannelHandle | None = None
    canonical_seed_cursor: int | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> LiveReplacementRequired:
        required = _boolean(data, "required", "live replacement result")
        if not required:
            _exact(data, {"required"}, "live replacement result")
            return cls(required=False)
        _exact(
            data,
            {"required", "reason", "replacement", "canonical_seed_cursor"},
            "live replacement result",
        )
        reason = _string(data, "reason", "live replacement result")
        if reason not in {"canonical_context", "delegation_result"}:
            raise ValueError("live replacement result.reason is unknown")
        replacement = data.get("replacement")
        if not isinstance(replacement, dict):
            raise ValueError("live replacement result.replacement must be an object")
        cursor = data.get("canonical_seed_cursor")
        if isinstance(cursor, bool) or not isinstance(cursor, int) or cursor < 0:
            raise ValueError(
                "live replacement result.canonical_seed_cursor must be a non-negative integer"
            )
        return cls(
            required=True,
            reason=reason,
            replacement=LiveChannelHandle.from_dict(replacement),
            canonical_seed_cursor=cursor,
        )

    def to_dict(self) -> dict[str, Any]:
        if not self.required:
            if any(
                value is not None
                for value in (self.reason, self.replacement, self.canonical_seed_cursor)
            ):
                raise ValueError("required=false cannot carry a replacement bootstrap")
            return {"required": False}
        if (
            self.reason is None
            or self.replacement is None
            or self.canonical_seed_cursor is None
        ):
            raise ValueError("required=true must carry a complete replacement bootstrap")
        return {
            "required": True,
            "reason": self.reason,
            "replacement": self.replacement.to_dict(),
            "canonical_seed_cursor": self.canonical_seed_cursor,
        }


@dataclass(frozen=True)
class LiveAssistantOutputAddress:
    """Opaque channel-scoped address published before assistant playback."""

    channel_id: str
    output_id: str
    content_index: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> LiveAssistantOutputAddress:
        _exact(
            data,
            {"channel_id", "output_id", "content_index"},
            "live assistant output address",
        )
        content_index = data.get("content_index")
        if (
            isinstance(content_index, bool)
            or not isinstance(content_index, int)
            or content_index < 0
        ):
            raise ValueError(
                "live assistant output address.content_index must be a non-negative integer"
            )
        return cls(
            channel_id=_string(data, "channel_id", "live assistant output address"),
            output_id=_string(data, "output_id", "live assistant output address"),
            content_index=content_index,
        )

    def to_dict(self) -> dict[str, Any]:
        if self.content_index < 0:
            raise ValueError(
                "live assistant output address.content_index must be a non-negative integer"
            )
        return {
            "channel_id": self.channel_id,
            "output_id": self.output_id,
            "content_index": self.content_index,
        }


@dataclass(frozen=True)
class LivePlaybackCompleteResult:
    status: Literal["completed"] = "completed"

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> LivePlaybackCompleteResult:
        _exact(data, {"status"}, "live playback complete result")
        if _string(data, "status", "live playback complete result") != "completed":
            raise ValueError("live playback complete result.status must be completed")
        return cls()

    def to_dict(self) -> dict[str, str]:
        if self.status != "completed":
            raise ValueError("live playback complete result.status must be completed")
        return {"status": "completed"}
