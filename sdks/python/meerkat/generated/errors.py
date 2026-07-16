"""Generated error types for Meerkat SDK."""

from __future__ import annotations

from typing import Any, Literal, NotRequired, Optional, Required, TypeGuard, TypedDict


class WireScopeDeniedDetail(TypedDict, total=False):
    presented: Required[list[Literal['list', 'read_history', 'subscribe_events', 'send_command', 'cancel', 'retire', 'wire_topology', 'live', 'admin_host', 'admin_grants']]]
    required: Required[Literal['list', 'read_history', 'subscribe_events', 'send_command', 'cancel', 'retire', 'wire_topology', 'live', 'admin_host', 'admin_grants']]

class WireHostUnavailableDetail(TypedDict, total=False):
    host: NotRequired[Optional[str]]
    timeout_ms: NotRequired[Optional[int]]

class WireStaleCursorDetail(TypedDict, total=False):
    generation: NotRequired[Optional[int]]
    requested: NotRequired[Optional[int]]
    watermark: Required[int]

class WireStaleFenceDetail(TypedDict, total=False):
    actual: NotRequired[Optional[int]]
    expected: NotRequired[Optional[int]]
    runtime_id: NotRequired[Optional[str]]

MultiHostErrorCode = Literal['SCOPE_DENIED', 'HOST_UNAVAILABLE', 'STALE_CURSOR', 'STALE_FENCE']

MULTI_HOST_JSON_RPC_ERROR_CODES: dict[int, MultiHostErrorCode] = {
    -32025: 'SCOPE_DENIED',
    -32026: 'HOST_UNAVAILABLE',
    -32027: 'STALE_CURSOR',
    -32028: 'STALE_FENCE',
}

def _is_scope_denied_detail(value: Any) -> TypeGuard[WireScopeDeniedDetail]:
    if not isinstance(value, dict):
        return False
    if not set(value).issubset(('presented', 'required')):
        return False
    return (
        ('presented' in value and (isinstance(value.get('presented'), list) and all(item in ('list', 'read_history', 'subscribe_events', 'send_command', 'cancel', 'retire', 'wire_topology', 'live', 'admin_host', 'admin_grants') for item in value.get('presented')))) and
        ('required' in value and value.get('required') in ('list', 'read_history', 'subscribe_events', 'send_command', 'cancel', 'retire', 'wire_topology', 'live', 'admin_host', 'admin_grants'))
    )

def _is_host_unavailable_detail(value: Any) -> TypeGuard[WireHostUnavailableDetail]:
    if not isinstance(value, dict):
        return False
    if not set(value).issubset(('host', 'timeout_ms')):
        return False
    return (
        ('host' not in value or (isinstance(value.get('host'), str) or value.get('host') is None)) and
        ('timeout_ms' not in value or ((isinstance(value.get('timeout_ms'), int) and not isinstance(value.get('timeout_ms'), bool) and value.get('timeout_ms') >= 0) or value.get('timeout_ms') is None))
    )

def _is_stale_cursor_detail(value: Any) -> TypeGuard[WireStaleCursorDetail]:
    if not isinstance(value, dict):
        return False
    if not set(value).issubset(('generation', 'requested', 'watermark')):
        return False
    return (
        ('generation' not in value or ((isinstance(value.get('generation'), int) and not isinstance(value.get('generation'), bool) and value.get('generation') >= 0) or value.get('generation') is None)) and
        ('requested' not in value or ((isinstance(value.get('requested'), int) and not isinstance(value.get('requested'), bool) and value.get('requested') >= 0) or value.get('requested') is None)) and
        ('watermark' in value and (isinstance(value.get('watermark'), int) and not isinstance(value.get('watermark'), bool) and value.get('watermark') >= 0))
    )

def _is_stale_fence_detail(value: Any) -> TypeGuard[WireStaleFenceDetail]:
    if not isinstance(value, dict):
        return False
    if not set(value).issubset(('actual', 'expected', 'runtime_id')):
        return False
    return (
        ('actual' not in value or ((isinstance(value.get('actual'), int) and not isinstance(value.get('actual'), bool) and value.get('actual') >= 0) or value.get('actual') is None)) and
        ('expected' not in value or ((isinstance(value.get('expected'), int) and not isinstance(value.get('expected'), bool) and value.get('expected') >= 0) or value.get('expected') is None)) and
        ('runtime_id' not in value or (isinstance(value.get('runtime_id'), str) or value.get('runtime_id') is None))
    )


class MeerkatError(Exception):
    """Base error for Meerkat SDK."""

    def __init__(self, code: str, message: str, details=None, capability_hint=None):
        super().__init__(message)
        self.code = code
        self.message = message
        self.details = details
        self.capability_hint = capability_hint


class CapabilityUnavailableError(MeerkatError):
    """Raised when a capability is not available."""
    pass


class SessionNotFoundError(MeerkatError):
    """Raised when a session is not found."""
    pass


class SkillNotFoundError(MeerkatError):
    """Raised when a skill reference cannot be resolved."""
    pass


class ScopeDeniedError(MeerkatError):
    """Raised for SCOPE_DENIED with validated typed details."""

    def __init__(
        self,
        message: str,
        details: WireScopeDeniedDetail,
        capability_hint: Any = None,
    ) -> None:
        super().__init__('SCOPE_DENIED', message, details, capability_hint)


class HostUnavailableError(MeerkatError):
    """Raised for HOST_UNAVAILABLE with validated typed details."""

    def __init__(
        self,
        message: str,
        details: WireHostUnavailableDetail,
        capability_hint: Any = None,
    ) -> None:
        super().__init__('HOST_UNAVAILABLE', message, details, capability_hint)


class StaleCursorError(MeerkatError):
    """Raised for STALE_CURSOR with validated typed details."""

    def __init__(
        self,
        message: str,
        details: WireStaleCursorDetail,
        capability_hint: Any = None,
    ) -> None:
        super().__init__('STALE_CURSOR', message, details, capability_hint)


class StaleFenceError(MeerkatError):
    """Raised for STALE_FENCE with validated typed details."""

    def __init__(
        self,
        message: str,
        details: WireStaleFenceDetail,
        capability_hint: Any = None,
    ) -> None:
        super().__init__('STALE_FENCE', message, details, capability_hint)


def meerkat_error_from_semantic_code(
    code: str,
    message: str,
    details: Any = None,
    capability_hint: Any = None,
) -> MeerkatError:
    if code == 'SCOPE_DENIED' and _is_scope_denied_detail(details):
        return ScopeDeniedError(message, details, capability_hint)
    if code == 'HOST_UNAVAILABLE' and _is_host_unavailable_detail(details):
        return HostUnavailableError(message, details, capability_hint)
    if code == 'STALE_CURSOR' and _is_stale_cursor_detail(details):
        return StaleCursorError(message, details, capability_hint)
    if code == 'STALE_FENCE' and _is_stale_fence_detail(details):
        return StaleFenceError(message, details, capability_hint)
    return MeerkatError(code, message, details, capability_hint)


def _normalize_jsonrpc_code(code: int | str) -> int | None:
    if isinstance(code, bool):
        return None
    if isinstance(code, int):
        return code
    if not isinstance(code, str):
        return None
    try:
        parsed = int(code)
    except ValueError:
        return None
    return parsed if str(parsed) == code else None


def meerkat_error_from_jsonrpc_code(
    rpc_code: int | str,
    semantic_code: str | None,
    message: str,
    details: Any = None,
    capability_hint: Any = None,
) -> MeerkatError:
    numeric_code = _normalize_jsonrpc_code(rpc_code)
    mapped_semantic = MULTI_HOST_JSON_RPC_ERROR_CODES.get(numeric_code)
    if semantic_code is not None:
        if mapped_semantic is not None and mapped_semantic != semantic_code:
            return MeerkatError(
                semantic_code, message, details, capability_hint
            )
        return meerkat_error_from_semantic_code(
            semantic_code, message, details, capability_hint
        )
    if mapped_semantic is not None:
        return meerkat_error_from_semantic_code(
            mapped_semantic, message, details, capability_hint
        )
    return MeerkatError(str(rpc_code), message, details, capability_hint)
