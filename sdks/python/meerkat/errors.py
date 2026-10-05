"""Meerkat SDK error types.

Single source of truth: the generated error hierarchy. This module
re-exports the generated classes so hand-written and generated code raise
the same exception types (K21 — the generated fail-closed parsers raise
``meerkat.generated.errors.MeerkatError``).
"""

from .generated.errors import (  # noqa: F401
    CapabilityUnavailableError as CapabilityUnavailableError,
    HostUnavailableError as HostUnavailableError,
    MULTI_HOST_JSON_RPC_ERROR_CODES as MULTI_HOST_JSON_RPC_ERROR_CODES,
    MeerkatError as MeerkatError,
    MultiHostErrorCode as MultiHostErrorCode,
    ScopeDeniedError as ScopeDeniedError,
    SessionNotFoundError as SessionNotFoundError,
    SkillNotFoundError as SkillNotFoundError,
    StaleCursorError as StaleCursorError,
    StaleFenceError as StaleFenceError,
    WireHostUnavailableDetail as WireHostUnavailableDetail,
    WireScopeDeniedDetail as WireScopeDeniedDetail,
    WireStaleCursorDetail as WireStaleCursorDetail,
    WireStaleFenceDetail as WireStaleFenceDetail,
    meerkat_error_from_jsonrpc_code as meerkat_error_from_jsonrpc_code,
    meerkat_error_from_semantic_code as meerkat_error_from_semantic_code,
)


from typing import Any as _Any, get_args as _get_args

from .generated.types import WireAuthErrorReason as WireAuthErrorReason


def _literal_values(alias: _Any) -> frozenset[str]:
    values: set[str] = set()
    for arm in _get_args(alias) or (alias,):
        args = _get_args(arm)
        if args and all(isinstance(value, str) for value in args):
            values.update(args)
        else:
            values.update(_literal_values(arm))
    return frozenset(values)


WIRE_AUTH_ERROR_REASONS: frozenset[str] = _literal_values(WireAuthErrorReason)


def auth_error_reason(error: BaseException) -> WireAuthErrorReason | None:
    """The typed reason of an auth error.

    ``auth/*`` RPC methods carry it in ``error.data.reason``, which the SDK
    keeps as ``MeerkatError.details``. Branch on it, never on the error text.
    ``None`` for errors without a known reason.
    """
    details = getattr(error, "details", None)
    if not isinstance(details, dict):
        return None
    reason = details.get("reason")
    if isinstance(reason, str) and reason in WIRE_AUTH_ERROR_REASONS:
        return reason  # type: ignore[return-value]
    return None
