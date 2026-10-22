"""Typed auth error reasons are readable without parsing error text."""

from typing import TYPE_CHECKING, get_args

from meerkat import MeerkatError, WIRE_AUTH_ERROR_REASONS, auth_error_reason
from meerkat.generated.types import WireAuthErrorReason

EXPECTED = {
    "invalid_target", "realm_not_found", "binding_not_found", "binding_invalid",
    "binding_inherited", "flow_unsupported", "mcp_server_not_configured",
    "mcp_server_mismatch", "account_selection_required", "unknown_strategy",
    "attempt_missing", "attempt_mismatch", "device_poll_in_progress",
    "device_code_already_admitted", "device_expiry_invalid", "account_mismatch",
    "missing_scopes", "credential_mismatch", "verification_unavailable",
    "slot_occupied", "slot_account_mismatch", "slot_context_mismatch",
    "slot_mode_mismatch", "unverified_connector_publication", "reauth_required",
    "authorization_required", "callback_unavailable", "upstream_failure",
    "configuration_invalid", "credential_access_denied",
    "credential_interaction_required", "credential_access_pending",
    "credential_store_unavailable", "infrastructure",
}


def test_the_reason_vocabulary_is_the_closed_server_list() -> None:
    assert WIRE_AUTH_ERROR_REASONS == EXPECTED
    assert get_args(WireAuthErrorReason)


def test_auth_error_reason_reads_details_only() -> None:
    for reason in EXPECTED:
        assert auth_error_reason(MeerkatError("-32602", "text", {"reason": reason})) == reason
    assert auth_error_reason(MeerkatError("-32602", "slot_occupied")) is None
    assert auth_error_reason(MeerkatError("-32602", "text", {"reason": "other"})) is None
    assert auth_error_reason(ValueError("x")) is None


if TYPE_CHECKING:
    from typing import assert_type

    def _narrowing(error: MeerkatError) -> bool:
        reason = auth_error_reason(error)
        assert_type(reason, WireAuthErrorReason | None)
        if reason == "slot_occupied":
            return True
        bad: WireAuthErrorReason = "slot_full"  # type: ignore[assignment]
        return bad == reason


def test_auth_error_detail_reads_the_typed_sub_kind_only() -> None:
    from meerkat import WIRE_AUTH_ERROR_DETAILS, auth_error_detail

    assert WIRE_AUTH_ERROR_DETAILS == {"remote_custody_required"}
    refused = MeerkatError(
        "-32602",
        "text",
        {"reason": "invalid_target", "detail": "remote_custody_required"},
    )
    assert auth_error_reason(refused) == "invalid_target"
    assert auth_error_detail(refused) == "remote_custody_required"
    assert auth_error_detail(MeerkatError("-32602", "text", {"reason": "invalid_target"})) is None
    assert auth_error_detail(MeerkatError("-32602", "text", {"detail": "other"})) is None
    assert auth_error_detail(MeerkatError("-32602", "remote_custody_required")) is None
    assert auth_error_detail(ValueError("x")) is None
