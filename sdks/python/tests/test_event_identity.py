"""Lifecycle identity is optional owner data, preserved by public SDK events."""

from copy import deepcopy
from typing import get_type_hints
import pytest
from meerkat import MeerkatClient, RunStarted, RunCompleted, RunFailed
from meerkat.events import parse_event, UnknownEvent

IDENTITY = {
    "interaction_id": "interaction-1",
    "run_id": "run-1",
    "objective_id": "objective-1",
    "realtime_origin": {
        "canonical_row_sequence": 7,
        "channel_id": "channel-1",
        "context_observation_id": {
            "channel_id": "channel-1",
            "namespace": "test",
            "nonce": "nonce-1",
        },
        "provider_item_ids": ["item-1", "item-2"],
        "session_id": "realtime-session",
    },
}
EVENTS = [
    ({"type": "run_started", "session_id": "session-1", "input": {"kind": "content", "content": "hello"}}, RunStarted),
    (
        {
            "type": "run_completed",
            "session_id": "session-1",
            "result": "done",
            "usage": {"input_tokens": 1, "output_tokens": 1},
        },
        RunCompleted,
    ),
    (
        {
            "type": "run_failed",
            "session_id": "session-1",
            "error_class": "internal",
            "error": "failed",
        },
        RunFailed,
    ),
]


@pytest.mark.parametrize("raw,cls", EVENTS)
def test_lifecycle_identity_absent_remains_absent(raw, cls):
    event = parse_event(raw)
    assert isinstance(event, cls)
    assert event.identity is None


@pytest.mark.parametrize("raw,cls", EVENTS)
@pytest.mark.parametrize(
    "identity",
    [
        {},
        IDENTITY,
        {
            "interaction_id": None,
            "run_id": None,
            "objective_id": None,
            "realtime_origin": None,
        },
        {
            "realtime_origin": {
                "canonical_row_sequence": 0,
                "channel_id": "c",
                "session_id": "s",
                "context_observation_id": None,
            }
        },
    ],
)
def test_lifecycle_identity_preserves_exact_owner_shape(raw, cls, identity):
    value = deepcopy(identity)
    event = parse_event({**raw, "identity": value})
    assert isinstance(event, cls)
    assert event.identity == identity
    if isinstance(identity.get("realtime_origin"), dict):
        assert event.identity["realtime_origin"]["session_id"] != raw["session_id"]
    envelope = MeerkatClient._parse_agent_event_envelope(
        {
            "event_id": "00000000-0000-4000-8000-000000000010",
            "source": {"type": "callback"},
            "seq": 0,
            "timestamp_ms": 1,
            "payload": {**raw, "identity": value},
        }
    )
    assert envelope.payload.identity == identity


INVALID = [
    None,
    "guessed",
    [],
    {"interaction_id": 7},
    {"run_id": False},
    {"objective_id": []},
    {"realtime_origin": {}},
    {"realtime_origin": "channel"},
]
for field, value in [
    ("canonical_row_sequence", -1),
    ("canonical_row_sequence", 1.5),
    ("canonical_row_sequence", True),
    ("channel_id", None),
    ("session_id", 7),
    ("provider_item_ids", None),
    ("provider_item_ids", [7]),
    ("context_observation_id", {}),
    ("context_observation_id", {"channel_id": "c", "namespace": "n", "nonce": 3}),
]:
    identity = deepcopy(IDENTITY)
    identity["realtime_origin"][field] = value
    INVALID.append(identity)


@pytest.mark.parametrize("raw,_cls", EVENTS)
@pytest.mark.parametrize("identity", INVALID)
def test_lifecycle_identity_invalid_nested_payload_is_not_laundered(
    raw, _cls, identity
):
    wire = {**raw, "identity": deepcopy(identity)}
    event = parse_event(wire)
    assert isinstance(event, UnknownEvent)
    assert event.type == "malformed_event"
    assert event.data == wire


def test_identity_support_types_are_public_generated_contracts():
    import meerkat
    from meerkat.generated import event_types

    for name in [
        "TranscriptMessageIdentity",
        "RealtimeMessageOrigin",
        "LiveContextObservationId",
        "ObjectiveId",
        "LiveChannelId",
        "RunId",
        "RunInput",
    ]:
        assert getattr(meerkat, name) is getattr(event_types, name)
    assert get_type_hints(RunStarted)["input"] == event_types.RunInput
    assert (
        get_type_hints(RunStarted)["identity"]
        == event_types.TranscriptMessageIdentity | None
    )


@pytest.mark.parametrize("run_input", [
    {"kind": "content", "content": "hello"},
    {"kind": "content", "content": [{"type": "text", "text": "hello"}]},
    {"kind": "pending_tool_results"},
])
def test_run_started_parses_runtime_input_without_fabricating_prompt(run_input):
    raw = {
        "type": "run_started",
        "session_id": "session-1",
        "input": deepcopy(run_input),
        "identity": deepcopy(IDENTITY),
    }
    event = parse_event(raw)
    assert isinstance(event, RunStarted)
    assert event.input == run_input
    assert event.identity == IDENTITY
    assert "prompt" not in event.__dataclass_fields__


@pytest.mark.parametrize("input_fields", [
    {},
    {"prompt": "old wire payload"},
    {"input": None},
    {"input": "hello"},
    {"input": {}},
    {"input": {"kind": "unknown"}},
    {"input": {"kind": "content"}},
    {"input": {"kind": "content", "content": None}},
    {"input": {"kind": "content", "content": 7}},
    {"input": {"kind": "content", "content": ["invalid block"]}},
])
def test_run_started_rejects_missing_or_malformed_runtime_input(input_fields):
    raw = {"type": "run_started", "session_id": "session-1", **input_fields}
    event = parse_event(raw)
    assert isinstance(event, UnknownEvent)
    assert event.type == "malformed_event"
    assert event.data == raw


@pytest.mark.parametrize("block", [
    {"type": "text", "text": "hello"},
    {"type": "image", "media_type": "image/png", "source": "inline", "data": "aGVsbG8="},
    {"type": "image", "media_type": "image/png", "source": "blob", "blob_id": "blob-1"},
    {"type": "video", "media_type": "video/mp4", "duration_ms": 12, "source": "inline", "data": "aGVsbG8="},
    {"type": "video", "media_type": "video/mp4", "duration_ms": 12, "source": "uri", "uri": "https://example.test/video.mp4"},
    {"type": "structured", "data": {"ready": True}},
    {"type": "structured", "data": None},
    {"type": "skill_context", "skill_key": {"source_uuid": "00000000-0000-4000-8000-000000000001", "skill_name": "test-skill"}, "text": "skill body"},
])
def test_run_started_preserves_each_core_content_block_variant(block):
    run_input = {"kind": "content", "content": [deepcopy(block)]}
    event = parse_event({"type": "run_started", "session_id": "s", "input": run_input})
    assert isinstance(event, RunStarted)
    assert event.input == run_input


@pytest.mark.parametrize("block", [
    {},
    {"type": "unknown"},
    {"type": "text"},
    {"type": "text", "text": 7},
    {"type": "image", "media_type": "image/png", "source": "inline"},
    {"type": "image", "media_type": "image/png", "source": "blob", "blob_id": 7},
    {"type": "image", "media_type": "image/png", "source": "uri", "uri": "https://example.test/image.png"},
    {"type": "video", "media_type": "video/mp4", "duration_ms": -1, "source": "inline", "data": "aGVsbG8="},
    {"type": "video", "media_type": "video/mp4", "duration_ms": True, "source": "uri", "uri": "https://example.test/video.mp4"},
    {"type": "video", "media_type": "video/mp4", "duration_ms": 1, "source": "uri", "uri": None},
    {"type": "structured"},
    {"type": "skill_context", "skill_key": {}, "text": "skill body"},
    {"type": "skill_context", "skill_key": {"sourceUuid": "00000000-0000-4000-8000-000000000001", "skillName": "test-skill"}, "text": "skill body"},
    {"type": "skill_context", "skill_key": {"source_uuid": "invalid", "skill_name": "test-skill"}, "text": "skill body"},
    {"type": "skill_context", "skill_key": {"source_uuid": "00000000-0000-4000-8000-000000000001", "skill_name": "Invalid--skill"}, "text": "skill body"},
])
def test_run_started_rejects_malformed_core_content_block_variants(block):
    raw = {"type": "run_started", "session_id": "s", "input": {"kind": "content", "content": [block]}}
    event = parse_event(raw)
    assert isinstance(event, UnknownEvent)
    assert event.type == "malformed_event"
    assert event.data == raw
