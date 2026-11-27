"""Typed presentation requests match the public Rust wire contract.

These are SDK serialization checks, not proof of presentation completeness or
authentication. The gateway retains both decisions.
"""

from dataclasses import replace
from types import MappingProxyType
from typing import get_args

import pytest

import meerkat_meta
from meerkat_meta import decisions
from .test_decisions import DECISION_ID, DIGEST, make_handle


def public_type(name):
    value = getattr(meerkat_meta, name, None)
    assert value is not None, f"missing public typed presentation export: {name}"
    assert value is getattr(decisions, name, None)
    assert name in meerkat_meta.__all__
    return value


def presentation(audience="approver", *, options=False):
    audience_value = (
        public_type("DecisionApproverAudience")("team:owners")
        if audience == "approver"
        else public_type("DecisionReviewerAudience")("principal:reviewer")
    )
    return public_type("DecisionPresentation")(
        audience=audience_value,
        subject_digest=DIGEST,
        parts_shown=[public_type("DecisionCanonicalPart")(0, 1, DIGEST)],
        channel_ref="chat:owners",
        message_ref="message-1",
        at_ms=0,
        max_part_bytes=0 if options else None,
        link=(
            public_type("DecisionPresentationLink")("document:1", DIGEST)
            if options else None
        ),
    )


def expected_wire(audience="approver", *, options=False):
    result = {
        "audience": (
            {"kind": "approver", "approver_id": "team:owners"}
            if audience == "approver"
            else {"kind": "reviewer", "reviewer": "principal:reviewer"}
        ),
        "subject_digest": DIGEST,
        "parts_shown": [{"index": 0, "count": 1, "part_digest": DIGEST}],
        "channel_ref": "chat:owners",
        "message_ref": "message-1",
        "at_ms": 0,
    }
    if options:
        result.update(max_part_bytes=0, link={"reference": "document:1", "served_digest": DIGEST})
    return result


def attestation(value):
    return decisions.DecisionAttestation(
        ingress="chat", subject="U123", scope="session-1",
        event_ref="event-1", observed_at_ms=1, presentation=value,
    )


@pytest.mark.asyncio
async def test_raw_presentations_remain_a_deliberate_unchanged_compatibility_path():
    # Raw dictionaries stay raw, including unknown/malformed fields. The
    # gateway, not the typed helper, owns their validation.
    raw = {"audience": {"kind": "future"}, "extra": "keep", "link": None}
    handle, calls = make_handle()
    await handle.decisions_present(DECISION_ID, MappingProxyType(raw))
    await handle.decisions_decide(
        DECISION_ID, approve=True, presented_digest=DIGEST,
        attestation=attestation(raw),
    )
    assert calls[0][1]["presentation"] == raw
    assert calls[1][1]["attestation"]["presentation"] == raw
    assert raw == {"audience": {"kind": "future"}, "extra": "keep", "link": None}


def test_typed_presentation_exports_and_audience_alias_are_public():
    for name in (
        "DecisionPresentation", "DecisionCanonicalPart", "DecisionPresentationLink",
        "DecisionApproverAudience", "DecisionReviewerAudience",
    ):
        public_type(name)
    assert set(get_args(public_type("DecisionAudience"))) == {
        public_type("DecisionApproverAudience"), public_type("DecisionReviewerAudience"),
    }


@pytest.mark.asyncio
@pytest.mark.parametrize("audience", ["approver", "reviewer"])
@pytest.mark.parametrize("options", [False, True])
async def test_both_nested_forms_send_the_exact_typed_presentation(audience, options):
    value = presentation(audience, options=options)
    wire = expected_wire(audience, options=options)
    assert value.to_dict() == wire
    handle, calls = make_handle()
    await handle.decisions_present(DECISION_ID, value)
    await handle.decisions_decide(
        DECISION_ID, approve=False, presented_digest=DIGEST,
        reason="reviewed", attestation=attestation(value),
    )
    assert calls == [
        ("meta/decisions/present", {"decision_id": DECISION_ID, "presentation": wire}),
        ("meta/decisions/decide", {
            "decision_id": DECISION_ID, "approve": False,
            "presented_digest": DIGEST, "reason": "reviewed",
            "attestation": {
                "ingress": "chat", "subject": "U123", "scope": "session-1",
                "event_ref": "event-1", "observed_at_ms": 1, "presentation": wire,
            },
        }),
    ]
    # The same valid wire value remains equivalent through the legacy path,
    # including both optional-field modes and the nested attestation.
    legacy_handle, legacy_calls = make_handle()
    await legacy_handle.decisions_present(DECISION_ID, wire)
    await legacy_handle.decisions_decide(
        DECISION_ID, approve=False, presented_digest=DIGEST,
        reason="reviewed", attestation=attestation(wire),
    )
    assert legacy_calls == calls


@pytest.mark.parametrize("name,kwargs", [
    ("DecisionApproverAudience", {}),
    ("DecisionApproverAudience", {"approver_id": 7}),
    ("DecisionApproverAudience", {"approver_id": "a", "reviewer": "r"}),
    ("DecisionReviewerAudience", {}),
    ("DecisionReviewerAudience", {"reviewer": False}),
    ("DecisionReviewerAudience", {"reviewer": "r", "approver_id": "a"}),
    ("DecisionCanonicalPart", {"index": 0, "count": 1}),
    ("DecisionCanonicalPart", {"index": 0, "count": 1, "part_digest": 7}),
    ("DecisionCanonicalPart", {"index": 0, "count": 1, "part_digest": "d", "extra": 1}),
    ("DecisionPresentationLink", {"reference": "r"}),
    ("DecisionPresentationLink", {"reference": "r", "served_digest": []}),
    ("DecisionPresentationLink", {"reference": "r", "served_digest": "d", "extra": 1}),
])
def test_missing_wrong_and_unknown_nested_fields_fail_locally(name, kwargs):
    constructor = public_type(name)
    with pytest.raises((TypeError, ValueError)):
        constructor(**kwargs)


@pytest.mark.parametrize("field,value", [
    ("audience", {"kind": "approver", "approver_id": "a"}),
    ("parts_shown", [{"index": 0, "count": 1, "part_digest": "d"}]),
    ("parts_shown", "parts"), ("link", {"reference": "r", "served_digest": "d"}),
    ("subject_digest", 4), ("channel_ref", None), ("message_ref", []),
    ("at_ms", True), ("at_ms", -1), ("at_ms", 1.5), ("at_ms", 2**64),
    ("max_part_bytes", True), ("max_part_bytes", -1), ("max_part_bytes", 2**32),
])
def test_typed_presentation_rejects_invalid_fields_before_dispatch(field, value):
    original = presentation()
    with pytest.raises((TypeError, ValueError)):
        replace(original, **{field: value})


@pytest.mark.parametrize("field", [
    "audience", "subject_digest", "parts_shown", "channel_ref", "message_ref", "at_ms",
])
def test_typed_presentation_requires_each_mandatory_field(field):
    original = presentation()
    kwargs = {
        "audience": original.audience, "subject_digest": DIGEST,
        "parts_shown": original.parts_shown, "channel_ref": "chat:owners",
        "message_ref": "message-1", "at_ms": 0,
    }
    del kwargs[field]
    with pytest.raises(TypeError):
        public_type("DecisionPresentation")(**kwargs)


def test_typed_presentation_rejects_unknown_top_level_fields():
    original = presentation()
    with pytest.raises(TypeError):
        replace(original, unrecognized="closed request")


@pytest.mark.parametrize("field", ["index", "count"])
@pytest.mark.parametrize("value", [True, -1, 1.5, 2**32])
def test_canonical_part_uint_fields_follow_the_rust_width(field, value):
    constructor = public_type("DecisionCanonicalPart")
    kwargs = {"index": 0, "count": 1, "part_digest": DIGEST, field: value}
    with pytest.raises((TypeError, ValueError)):
        constructor(**kwargs)


def test_structural_typing_does_not_invent_gateway_semantic_validation():
    value = replace(
        presentation(), subject_digest="gateway-validates-digests", at_ms=2**64 - 1,
        parts_shown=[public_type("DecisionCanonicalPart")(2**32 - 1, 0, "d")],
        max_part_bytes=2**32 - 1,
    )
    wire = value.to_dict()
    assert wire["at_ms"] == 2**64 - 1
    assert wire["parts_shown"] == [{"index": 2**32 - 1, "count": 0, "part_digest": "d"}]
    assert wire["max_part_bytes"] == 2**32 - 1
    assert "link" not in wire


def test_omitted_and_none_options_serialize_identically_without_mutation():
    value = presentation()
    constructor = public_type("DecisionPresentation")
    omitted = constructor(
        audience=value.audience, subject_digest=DIGEST, parts_shown=value.parts_shown,
        channel_ref="chat:owners", message_ref="message-1", at_ms=0,
    )
    first = omitted.to_dict()
    first["parts_shown"][0]["part_digest"] = "changed-output"
    assert omitted.to_dict() == value.to_dict() == expected_wire()
