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
    StaleDeliveryScopeError as StaleDeliveryScopeError,
    StaleFenceError as StaleFenceError,
    WireHostUnavailableDetail as WireHostUnavailableDetail,
    WireScopeDeniedDetail as WireScopeDeniedDetail,
    WireStaleCursorDetail as WireStaleCursorDetail,
    WireStaleDeliveryScopeDetail as WireStaleDeliveryScopeDetail,
    WireStaleFenceDetail as WireStaleFenceDetail,
    meerkat_error_from_jsonrpc_code as meerkat_error_from_jsonrpc_code,
    meerkat_error_from_semantic_code as meerkat_error_from_semantic_code,
)


from typing import Any as _Any, get_args as _get_args, get_type_hints as _get_type_hints

from .generated.types import (
    CommsSendErrorData as CommsSendErrorData,
    WireAuthErrorDetail as WireAuthErrorDetail,
    WireAuthErrorReason as WireAuthErrorReason,
)


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


def _literal_strings(alias: _Any) -> frozenset[str]:
    # A single `Literal[...]` (its arguments are the strings), or a union of
    # them.
    args = _get_args(alias)
    if args and all(isinstance(value, str) for value in args):
        return frozenset(args)
    return _literal_values(alias)


WIRE_AUTH_ERROR_DETAILS: frozenset[str] = _literal_strings(WireAuthErrorDetail)


def auth_error_detail(error: BaseException) -> WireAuthErrorDetail | None:
    """The stable sub-kind of an auth refusal, beside its reason.

    ``auth/*`` RPC methods carry it in ``error.data.detail``. For example
    ``"remote_custody_required"`` (reason ``"invalid_target"``): an MCP login
    started on the host's registered https callback was completed without
    its ``host_attempt_handle``, for example after a restart; cancel the
    attempt by its reference and start again. ``None`` when absent.
    """
    details = getattr(error, "details", None)
    if not isinstance(details, dict):
        return None
    detail = details.get("detail")
    if isinstance(detail, str) and detail in WIRE_AUTH_ERROR_DETAILS:
        return detail  # type: ignore[return-value]
    return None


COMMS_SEND_ERROR_CODES: frozenset[str] = frozenset().union(
    *(
        _literal_values(_get_type_hints(arm, include_extras=True)["code"])
        for arm in _get_args(CommsSendErrorData)
    )
)


def comms_send_error(error: BaseException) -> CommsSendErrorData | None:
    """The typed data of a failed ``comms/send``.

    ``comms/send`` carries it in ``error.data``, tagged on ``code``, which the
    SDK keeps as ``MeerkatError.details``. Branch on ``code``, never on the
    error text. ``None`` for errors without a known ``comms/send`` code.
    """
    details = getattr(error, "details", None)
    if not isinstance(details, dict):
        return None
    code = details.get("code")
    if isinstance(code, str) and code in COMMS_SEND_ERROR_CODES:
        return details  # type: ignore[return-value]
    return None


class BinaryIntegrityError(MeerkatError):
    """A downloaded or cached ``rkat-rpc`` does not match its release digest.

    Raised with code ``BINARY_INTEGRITY_MISMATCH`` when the auto-downloaded
    release archive, the binary inside it, or the cached binary does not match
    the sha256 this package pins (stamped at release time). The SDK removes
    what failed, never runs it and never falls back to another binary.
    ``path`` names the cached binary, or is ``None`` for a download;
    ``details`` carries the same fields.
    """

    def __init__(
        self,
        message: str,
        *,
        asset: str,
        path: str | None,
        expected_sha256: str,
        actual_sha256: str,
    ) -> None:
        super().__init__(
            "BINARY_INTEGRITY_MISMATCH",
            message,
            details={
                "asset": asset,
                "path": path,
                "expected_sha256": expected_sha256,
                "actual_sha256": actual_sha256,
            },
        )
        self.asset = asset
        self.path = path
        self.expected_sha256 = expected_sha256
        self.actual_sha256 = actual_sha256
