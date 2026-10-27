"""meerkat's typed reload-required refusal from ``meta/force_cancel_member``.

The gateway answers a member whose runtime registration must be reloaded
with meerkat's own code (-32029) and data naming the member and the repair;
the SDK raises :class:`MemberReloadRequiredError` carrying those facts.
"""
from __future__ import annotations

import meerkat_meta
from meerkat_meta import (
    MEMBER_RELOAD_REQUIRED_CODE,
    MEMBER_RELOAD_REQUIRED_KIND,
    MemberReloadRequiredError,
    RpcError,
)
from meerkat_meta.runtime import _rpc_error_from_payload


def test_is_a_typed_rpc_error_exported_at_top_level() -> None:
    assert issubclass(MemberReloadRequiredError, RpcError)
    assert meerkat_meta.MemberReloadRequiredError is MemberReloadRequiredError
    assert meerkat_meta.MEMBER_RELOAD_REQUIRED_CODE == MEMBER_RELOAD_REQUIRED_CODE == -32029
    assert (
        meerkat_meta.MEMBER_RELOAD_REQUIRED_KIND
        == MEMBER_RELOAD_REQUIRED_KIND
        == "mob_member_reload_required"
    )


def test_a_refusal_reifies_with_the_member_and_the_repair() -> None:
    error = _rpc_error_from_payload(
        {
            "code": -32029,
            "message": "member worker requires a runtime reload before it can accept work",
            "data": {
                "kind": "mob_member_reload_required",
                "member_id": "worker",
                "reason": "runtime registration is degraded",
                "retryable": False,
                "authority_retained": True,
                "required_action": "reload_member_registration",
            },
        },
        request_id="force-cancel:1",
        method="meta/force_cancel_member",
    )
    assert isinstance(error, MemberReloadRequiredError)
    assert error.code == -32029
    assert error.member_id == "worker"
    assert error.reason == "runtime registration is degraded"
    assert error.required_action == "reload_member_registration"
    assert error.data["kind"] == "mob_member_reload_required"
    assert error.method == "meta/force_cancel_member"


def test_a_refusal_without_fields_still_reifies() -> None:
    error = _rpc_error_from_payload(
        {
            "code": -32029,
            "message": "member requires a runtime reload",
            "data": {"kind": "mob_member_reload_required"},
        },
        request_id="force-cancel:1",
        method="meta/force_cancel_member",
    )
    assert isinstance(error, MemberReloadRequiredError)
    assert error.member_id is None
    assert error.reason is None
    assert error.required_action is None


def test_the_code_without_the_kind_stays_a_plain_rpc_error() -> None:
    for data in (None, {}, {"kind": "something_else", "member_id": "worker"}, "text"):
        payload = {"code": -32029, "message": "member requires a runtime reload"}
        if data is not None:
            payload["data"] = data
        error = _rpc_error_from_payload(
            payload,
            request_id="force-cancel:1",
            method="meta/force_cancel_member",
        )
        assert not isinstance(error, MemberReloadRequiredError)
        assert type(error) is RpcError
        assert error.code == -32029


def test_other_codes_do_not_reify_as_a_reload_refusal() -> None:
    for code in (-32000, -32602):
        error = _rpc_error_from_payload(
            {
                "code": code,
                "message": "refused",
                "data": {"kind": "mob_member_reload_required", "member_id": "worker"},
            },
            request_id="force-cancel:1",
            method="meta/force_cancel_member",
        )
        assert not isinstance(error, MemberReloadRequiredError)
        assert error.code == code
