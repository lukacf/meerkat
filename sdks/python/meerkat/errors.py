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
    WireMemberReloadRefusal as WireMemberReloadRefusal,
    WireMemberReloadRefusalRepairBlocked as WireMemberReloadRefusalRepairBlocked,
    WireMemberReloadRefusalUnknown as WireMemberReloadRefusalUnknown,
    WireMobDestroyMemberRefusal as WireMobDestroyMemberRefusal,
    WireMobDestroyReport as WireMobDestroyReport,
)


class EventSubscriptionError(MeerkatError):
    """A standalone subscription ended with a server-reported error.

    Earlier queued events are delivered before this error is raised. The raw
    terminal notification is retained, including unknown codes and outcomes.
    Sequence fields describe the last delivered event, not a replay cursor.
    """

    def __init__(
        self,
        stream_id: str,
        terminal_outcome: dict[str, _Any],
        last_rpc_sequence: int | None,
        last_event_seq: int | None,
    ):
        error = terminal_outcome.get("error")
        if (
            isinstance(error, dict)
            and isinstance(error.get("code"), str)
            and isinstance(error.get("message"), str)
        ):
            code, message = error["code"], error["message"]
        else:
            code, message = "INVALID_RESPONSE", "Malformed stream_end error"
        super().__init__(
            code,
            message,
            details={
                "stream_id": stream_id,
                "terminal_outcome": terminal_outcome,
                "last_rpc_sequence": last_rpc_sequence,
                "last_event_seq": last_event_seq,
            },
        )
        self.stream_id = stream_id
        self.terminal_outcome = terminal_outcome
        self.last_rpc_sequence = last_rpc_sequence
        self.last_event_seq = last_event_seq


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
    """The stable sub-kind of an auth error, beside its reason.

    ``auth/*`` RPC methods carry it in ``error.data.detail``. For example
    ``"remote_custody_required"`` (reason ``"invalid_target"``): an MCP login
    started on the host's registered https callback was completed without
    its ``host_attempt_handle``, for example after a restart; cancel the
    attempt by its reference and start again. ``"upstream_timeout"`` and
    ``"upstream_timeout_after_code"`` (reason ``"upstream_failure"``): a
    network stage of an MCP login got no answer within its bound, before or
    after the authorization code was sent; nothing was published, so start
    again. ``None`` when absent.
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


def mob_destroy_incomplete_report(error: BaseException) -> WireMobDestroyReport | None:
    """The destroy report of a mob destroy that did not finish.

    ``mob/destroy``, and ``mob/lifecycle`` with ``"destroy"``, fail with
    ``error.data`` ``{"code": "mob_destroy_incomplete", "destroy_report": ...,
    "retryable": true}``, which the SDK keeps as ``MeerkatError.details``. The
    report says what is left; for example, ``member_reload_refusals`` names
    each member whose reload the runtime refused, with its typed ``refusal``:
    retry the destroy later. A refusal ``kind`` this SDK does not know reads
    as ``{"kind": "unknown"}``. ``None`` for any other error, or when the
    report does not parse.
    """
    details = getattr(error, "details", None)
    if not isinstance(details, dict) or details.get("code") != "mob_destroy_incomplete":
        return None
    return _parse_mob_destroy_report(details.get("destroy_report"))


_DESTROY_REPORT_STRING_LISTS = (
    "force_destroyed_members",
    "orphaned_remote_members",
    "errors",
    "reloaded_members",
)
_DESTROY_REPORT_FLAGS = (
    "remote_cleanup_deadline_exceeded",
    "metadata_scrubbed",
    "events_cleared",
    "namespace_cleaned",
)


def _parse_mob_destroy_report(value: _Any) -> WireMobDestroyReport | None:
    """The report's known fields, each checked; absent fields stay ``None``."""
    if not isinstance(value, dict):
        return None
    fields: dict[str, _Any] = {}
    for name in _DESTROY_REPORT_STRING_LISTS:
        if name in value:
            items = value[name]
            if not isinstance(items, list) or not all(isinstance(item, str) for item in items):
                return None
            fields[name] = list(items)
    for name in _DESTROY_REPORT_FLAGS:
        if name in value:
            if not isinstance(value[name], bool):
                return None
            fields[name] = value[name]
    if "member_reload_refusals" in value:
        entries = value["member_reload_refusals"]
        if not isinstance(entries, list):
            return None
        refusals = [_parse_mob_destroy_member_refusal(entry) for entry in entries]
        if any(refusal is None for refusal in refusals):
            return None
        fields["member_reload_refusals"] = refusals
    return WireMobDestroyReport(**fields)


def _parse_mob_destroy_member_refusal(value: _Any) -> WireMobDestroyMemberRefusal | None:
    if not isinstance(value, dict) or not isinstance(value.get("member"), str):
        return None
    refusal = _parse_member_reload_refusal(value.get("refusal"))
    if refusal is None:
        return None
    return WireMobDestroyMemberRefusal(member=value["member"], refusal=refusal)


def _parse_member_reload_refusal(value: _Any) -> WireMemberReloadRefusal | None:
    if not isinstance(value, dict) or not isinstance(value.get("kind"), str):
        return None
    if value["kind"] != "repair_blocked":
        # A kind this SDK does not know: the server's own `Unknown` form.
        unknown: WireMemberReloadRefusalUnknown = {"kind": "unknown"}
        return unknown
    reason = value.get("reason")
    digest = value.get("evidence_digest")
    if not isinstance(reason, str) or not (digest is None or isinstance(digest, str)):
        return None
    refusal: WireMemberReloadRefusalRepairBlocked = {"kind": "repair_blocked", "reason": reason}
    if digest is not None:
        refusal["evidence_digest"] = digest
    return refusal


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
