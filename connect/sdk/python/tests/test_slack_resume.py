"""Public resumable traversal: each returned part owns only its own observations."""
from copy import deepcopy

import httpx
import pytest

from test_slack_interval import DEFAULT, page, selected

pytestmark = pytest.mark.asyncio


def rows(result):
    return [row["message"] for row in result["messages"]]


async def test_parts_preserve_queue_and_equal_uninterrupted_occurrences():
    pages = [
        page([{"ts": "1"}], response_metadata={"next_cursor": "next"}),
        page([{"ts": "11", "thread_ts": "1"}]),
        page([{"ts": "12"}], response_metadata={"next_cursor": "last"}),
        page([{"ts": "12"}, {"ts": "13", "thread_ts": "12"}]),
        page(),
    ]
    client, slack, requests = selected(pages)
    parts = []
    async with client:
        state = None
        for _ in pages:
            part = await slack.read_interval("C1", **{**DEFAULT, "max_requests": 1},
                                             resumable=True, continuation=state)
            parts.append(part)
            state = part["continuation"]
    whole_client, whole_slack, whole_requests = selected(pages)
    async with whole_client:
        whole = await whole_slack.read_interval("C1", **DEFAULT)
    assert [row for part in parts for row in rows(part)] == rows(whole)
    assert [str(r.url) for r in requests] == [str(r.url) for r in whole_requests]
    assert state is None
    assert all(p["requests_made"] == 1 and p["observation_scope"] == "current_call_only" for p in parts)
    assert "continuation" not in whole and "observation_scope" not in whole


async def test_rate_limit_retries_only_failed_page_and_resets_observations():
    client, slack, requests = selected([
        page([{"ts": "1"}]),
        httpx.Response(429, headers={"retry-after": "60"}),
        page([{"ts": "11", "thread_ts": "1"}]),
    ])
    async with client:
        first = await slack.read_interval("C1", **DEFAULT, resumable=True)
        checkpoint = deepcopy(first["continuation"])
        second = await slack.read_interval("C1", **DEFAULT, resumable=True, continuation=checkpoint)
    assert len(requests) == 3
    assert first["termination"] == "request_failure"
    assert first["threads"][0]["error"]["retry_after_seconds"] == 60
    assert second["history"] == {"state": "not_requested", "pages": 0, "stop_reason": None}
    assert second["threads"][0]["pages"] == 1 and "error" not in second["threads"][0]
    assert rows(second) == [{"ts": "11", "thread_ts": "1"}]
    assert second["continuation"] is None and checkpoint == first["continuation"]


async def test_rejected_message_page_is_retried_atomically_with_larger_budget():
    overflow = page([{"ts": "11"}, {"ts": "12"}], is_limited=True)
    client, slack, requests = selected([overflow, overflow, page(), page()])
    async with client:
        first = await slack.read_interval("C1", **{**DEFAULT, "max_messages": 1}, resumable=True)
        second = await slack.read_interval("C1", **DEFAULT, resumable=True, continuation=first["continuation"])
    assert first["messages"] == [] and first["threads"] == []
    assert first["history"]["pages"] == 0 and first["termination"] == "message_budget"
    assert first["continuation"]["discovered_roots"] == []
    assert [gap["kind"] for gap in first["coverage_gaps"]] == ["message_budget"]
    assert len(rows(second)) == 2 and second["continuation"] is None
    assert str(requests[0].url) == str(requests[1].url)


async def test_cycle_spans_calls_and_does_not_invent_terminal_history_observation():
    client, slack, requests = selected([
        page(response_metadata={"next_cursor": "A"}),
        page(response_metadata={"next_cursor": "B"}),
        page(response_metadata={"next_cursor": "A"}),
    ])
    async with client:
        state = None
        for _ in range(3):
            result = await slack.read_interval("C1", **{**DEFAULT, "max_requests": 1},
                                               resumable=True, continuation=state)
            state = result["continuation"]
    assert result["history"]["pages"] == 1
    assert result["history"]["stop_reason"] == "cursor_cycle" and state is None
    assert [r.url.params.get("cursor") for r in requests] == [None, "A", "B"]


async def test_changed_query_rejected_before_provider_and_token_not_mutated():
    client, slack, requests = selected([page(response_metadata={"next_cursor": "A"})])
    async with client:
        first = await slack.read_interval("C1", **{**DEFAULT, "max_requests": 1}, resumable=True)
        checkpoint = deepcopy(first["continuation"])
        for changes in ({"start": "10.0"}, {"end": "21"}, {"root_oldest": "1"},
                        {"page_limit": 16}, {"include_all_metadata": True}):
            with pytest.raises(ValueError):
                await slack.read_interval("C1", **{**DEFAULT, **changes},
                                          resumable=True, continuation=checkpoint)
        with pytest.raises(ValueError):
            await slack.read_interval("C1", **DEFAULT, continuation=checkpoint)
    assert len(requests) == 1 and checkpoint == first["continuation"]
