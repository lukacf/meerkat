"""Runtime boundary application facts survive the public SDK event decoders."""

from copy import deepcopy

import pytest

from meerkat import MeerkatClient
from meerkat.events import ScopedEvent, UnknownEvent, parse_event


SESSION_ID = "00000000-0000-4000-8000-000000000001"
RUN_ID = "00000000-0000-4000-8000-000000000002"
INPUT_ID = "00000000-0000-4000-8000-000000000003"
NOTICE = {
    "kind": "background_job",
    "body": "Image generation finished.",
    "blocks": [{
        "type": "background_job",
        "job_id": "job-image-1",
        "status": "completed",
        "persisted": True,
    }],
    "created_at": "2026-09-26T00:00:00Z",
    "runtime_origin": {
        "session_id": SESSION_ID,
        "run_id": RUN_ID,
        "input_id": INPUT_ID,
        "append_ordinal": 1,
    },
}
EVENTS = [
    pytest.param({
        "type": "boundary_append_applied",
        "run_id": RUN_ID,
        "input_id": INPUT_ID,
        "content": [{"type": "text", "text": "Image generation finished."}],
        "append_count": 2,
        "notices": [NOTICE],
        "transcript_start": 7,
    }, id="applied-with-notice-provenance"),
    pytest.param({
        "type": "boundary_append_applied",
        "run_id": RUN_ID,
        "input_id": INPUT_ID,
        "content": "Legacy append without notice metadata.",
        "append_count": 1,
    }, id="applied-without-optional-fields"),
    pytest.param({
        "type": "boundary_appends_discarded",
        "session_id": SESSION_ID,
        "run_id": RUN_ID,
        "input_ids": [INPUT_ID, "00000000-0000-4000-8000-000000000004"],
    }, id="discarded-with-exact-inputs"),
]


def assert_preserved(event, raw):
    # These generated-schema events use the SDK's explicit known-event
    # passthrough until dedicated ergonomic parser classes are provided.
    assert isinstance(event, UnknownEvent)
    assert event.type == raw["type"]
    assert event.data == raw


@pytest.mark.parametrize("raw", EVENTS)
def test_boundary_append_events_parse_without_losing_runtime_fields(raw):
    assert_preserved(parse_event(deepcopy(raw)), raw)


@pytest.mark.parametrize("raw", EVENTS)
def test_boundary_append_events_survive_envelope_and_scoped_decoding(raw):
    envelope = MeerkatClient._parse_agent_event_envelope({
        "event_id": "00000000-0000-4000-8000-000000000010",
        "source": {"type": "session", "session_id": SESSION_ID},
        "seq": 7,
        "timestamp_ms": 1,
        "payload": deepcopy(raw),
    })
    assert_preserved(envelope.payload, raw)

    scoped = parse_event({
        "scope_id": "primary",
        "scope_path": [{"scope": "primary", "session_id": SESSION_ID}],
        "event": deepcopy(raw),
    })
    assert isinstance(scoped, ScopedEvent)
    assert_preserved(scoped.event, raw)
