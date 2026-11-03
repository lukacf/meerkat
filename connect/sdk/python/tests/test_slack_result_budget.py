"""Whole-result byte limits retain exact pages and existing continuation ownership."""
from copy import deepcopy
import json

import httpx
import pytest

from meerkat_toolkit.connectors.slack import (
    DEFAULT_INTERVAL_RESULT_BYTES, MAX_INTERVAL_RESULT_BYTES, MIN_INTERVAL_RESULT_BYTES,
)
from test_slack_interval import DEFAULT, page, selected


pytestmark = pytest.mark.asyncio


def size(value):
    return len(json.dumps(value, ensure_ascii=True, allow_nan=False,
                          separators=(",", ":")).encode("utf-8"))


def row(text, ts="11"):
    # This legitimate non-threadable subtype isolates a history page's size.
    return {"ts": ts, "text": text, "subtype": "channel_join"}


def assert_bounded(result, maximum):
    assert result["limits"]["max_result_bytes"] == maximum
    assert size(result) <= maximum


@pytest.mark.parametrize("maximum", [False, True, None, 0, 4095, 16777217, 4096.0, "4096"])
async def test_invalid_result_budget_precedes_provider(maximum):
    client, slack, calls = selected([])
    async with client:
        with pytest.raises(ValueError, match="max_result_bytes"):
            await slack.read_interval("C1", **DEFAULT, max_result_bytes=maximum)
    assert calls == []


async def test_default_and_inclusive_endpoints_bound_complete_result():
    assert (MIN_INTERVAL_RESULT_BYTES, DEFAULT_INTERVAL_RESULT_BYTES,
            MAX_INTERVAL_RESULT_BYTES) == (4096, 262144, 16777216)
    for maximum in (None, MIN_INTERVAL_RESULT_BYTES, MAX_INTERVAL_RESULT_BYTES):
        client, slack, calls = selected([page([row("small")])])
        async with client:
            kwargs = {} if maximum is None else {"max_result_bytes": maximum}
            result = await slack.read_interval("C1", **DEFAULT, resumable=True, **kwargs)
        assert len(calls) == 1 and result["termination"] == "finished"
        assert_bounded(result, maximum or DEFAULT_INTERVAL_RESULT_BYTES)


async def test_exact_fit_and_one_byte_over_complete_terminal_page():
    payload = page([row("x" * 5000)], is_limited=True)
    client, slack, _ = selected([payload])
    async with client:
        expected = await slack.read_interval("C1", **DEFAULT, resumable=True, max_result_bytes=100000)
    maximum = size(expected)
    while expected["limits"]["max_result_bytes"] != maximum:
        expected["limits"]["max_result_bytes"] = maximum
        maximum = size(expected)
    for budget, accepted in ((maximum, True), (maximum - 1, False)):
        client, slack, calls = selected([payload])
        async with client:
            actual = await slack.read_interval("C1", **DEFAULT, resumable=True, max_result_bytes=budget)
        assert len(calls) == 1
        assert_bounded(actual, budget)
        assert (actual["termination"] == "finished") is accepted
        if accepted:
            assert actual == expected and size(actual) == maximum
        else:
            assert actual["messages"] == [] and actual["history"]["pages"] == 0
            assert actual["coverage_gaps"] == [{"kind": "result_budget", "source": "history"}]


async def test_prior_page_survives_without_new_rows_roots_gaps_or_cursor():
    first = page([row("a" * 1400)], has_more=True, is_limited=True,
                 response_metadata={"next_cursor": "second"})
    refused = page([{"ts": "12", "text": "b" * 3300}], is_limited=True)
    original_first, original_refused = deepcopy(first), deepcopy(refused)
    client, slack, calls = selected([first, refused, refused, page()])
    async with client:
        part = await slack.read_interval("C1", **DEFAULT, resumable=True, max_result_bytes=4096)
        checkpoint = deepcopy(part["continuation"])
        resumed = await slack.read_interval("C1", **DEFAULT, resumable=True,
            continuation=checkpoint, max_result_bytes=8192)
    assert_bounded(part, 4096)
    assert part["termination"] == "result_budget" and part["requests_made"] == 2
    assert [r["message"] for r in part["messages"]] == original_first["messages"]
    assert part["threads"] == [] and checkpoint["discovered_roots"] == []
    assert part["history"] == {"state": "stopped", "pages": 1, "stop_reason": "result_budget",
                               "next_cursor": "second", "has_more": True}
    assert part["coverage_gaps"] == [{"kind": "retention_limited", "source": "history"},
                                     {"kind": "result_budget", "source": "history"}]
    assert checkpoint["pending"] == [{"root_ts": None, "cursor": "second", "seen_cursors": ["second"]}]
    assert [r["message"] for r in resumed["messages"]] == original_refused["messages"]
    assert resumed["continuation"] is None and resumed["termination"] == "finished"
    assert_bounded(resumed, 8192)
    assert checkpoint == part["continuation"]
    assert str(calls[1].url) == str(calls[2].url) and len(calls) == 4
    assert first == original_first and refused == original_refused


async def test_oversized_first_message_stalls_without_automatic_retry():
    payload = page([row("z" * 5000)])
    client, slack, calls = selected([payload, payload])
    async with client:
        part = await slack.read_interval("C1", **DEFAULT, resumable=True, max_result_bytes=4096)
        assert len(calls) == 1
        assert_bounded(part, 4096)
        assert part["messages"] == [] and part["history"]["pages"] == 0
        assert part["continuation"]["pending"] == [{"root_ts": None, "cursor": None, "seen_cursors": []}]
        assert "max_result_bytes" not in part["continuation"]["selection"]
        with pytest.raises(ValueError):
            await slack.read_interval("C1", **DEFAULT, resumable=True, max_result_bytes=8192,
                                      continuation=part["continuation"], page_limit=1)
        assert len(calls) == 1
        resumed = await slack.read_interval("C1", **DEFAULT, resumable=True, max_result_bytes=8192,
                                           continuation=part["continuation"])
    assert_bounded(resumed, 8192)
    assert resumed["messages"][0]["message"] == payload["messages"][0]
    assert resumed["termination"] == "finished" and len(calls) == 2


async def test_nonresumable_overflow_has_visible_gap_without_checkpoint():
    client, slack, calls = selected([page([row("x" * 5000)])])
    async with client:
        result = await slack.read_interval("C1", **DEFAULT, max_result_bytes=4096)
    assert_bounded(result, 4096)
    assert result["termination"] == "result_budget" and result["messages"] == []
    assert result["coverage_gaps"] and len(calls) == 1
    assert "continuation" not in result and "observation_scope" not in result


@pytest.mark.parametrize("payload", [
    page([row("\u4e2d" * 1100)]),
    page([row(('\x7f"\\\n') * 500)]),
    page(response_metadata={"next_cursor": "\u00e9" * 1000}),
])
async def test_unicode_escaping_and_metadata_count_even_without_messages(payload):
    client, slack, calls = selected([payload])
    async with client:
        result = await slack.read_interval("C1", **DEFAULT, resumable=True, max_result_bytes=4096)
    assert_bounded(result, 4096)
    assert len(calls) == 1 and result["termination"] == "result_budget"
    assert result["messages"] == [] and result["history"]["pages"] == 0
    assert result["continuation"]["pending"][0]["cursor"] is None


async def test_large_existing_checkpoint_refuses_before_http():
    cursor = "c" * 5000
    checkpoint = {"schema_version": 1,
        "selection": {"channel": "C1", "start": "10", "end": "20", "root_oldest": "0",
                      "page_limit": 15, "include_all_metadata": False},
        "pending": [{"root_ts": None, "cursor": cursor, "seen_cursors": [cursor]}],
        "discovered_roots": []}
    original = deepcopy(checkpoint)
    client, slack, calls = selected([])
    async with client:
        with pytest.raises(ValueError, match="bounded stop"):
            await slack.read_interval("C1", **DEFAULT, resumable=True, continuation=checkpoint,
                                      max_result_bytes=4096)
    assert calls == [] and checkpoint == original


async def test_failure_tail_and_retry_after_fit_without_erasing_prior_page():
    client, slack, calls = selected([
        page([row("x" * 2600)], response_metadata={"next_cursor": "later"}),
        httpx.Response(429, headers={"retry-after": "999999999"}),
    ])
    async with client:
        result = await slack.read_interval("C1", **DEFAULT, resumable=True, max_result_bytes=4096)
    assert_bounded(result, 4096)
    assert len(calls) == 2 and result["termination"] == "request_failure"
    assert len(result["messages"]) == 1 and result["history"]["pages"] == 1
    assert result["history"]["error"]["retry_after_seconds"] == 999999999
    assert result["continuation"]["pending"][0]["cursor"] == "later"


async def test_page_must_leave_space_for_the_next_complete_failure_return():
    payload = page([row("x" * 4000)], response_metadata={"next_cursor": "later"})
    client, slack, _ = selected([payload])
    async with client:
        candidate = await slack.read_interval("C1", **{**DEFAULT, "max_requests": 1},
                                             resumable=True, max_result_bytes=100000)
    # Reconstruct that admitted, not-yet-stopped candidate. It fits this budget,
    # but the next provider's full error/stop result would not.
    candidate["limits"]["max_requests"] = 2
    candidate["termination"] = "finished"
    candidate["coverage_gaps"] = []
    maximum = size(candidate) + 32
    for _ in range(3):
        candidate["limits"]["max_result_bytes"] = maximum
        maximum = size(candidate) + 32
    assert maximum > 4096 and size(candidate) < maximum
    client, slack, calls = selected([payload])
    async with client:
        result = await slack.read_interval("C1", **{**DEFAULT, "max_requests": 2},
                                          resumable=True, max_result_bytes=maximum)
    assert_bounded(result, maximum)
    assert len(calls) == 1 and result["termination"] == "result_budget"
    assert result["messages"] == [] and result["history"]["pages"] == 0
    assert result["continuation"]["pending"][0]["cursor"] is None


async def test_unusual_host_error_cannot_overrun_budget_or_advance_failed_page():
    # Real HTTP status codes are three digits. A custom host transport can
    # supply a larger integer; do not let that unexpected diagnostic defeat
    # the full-result bound or silently erase the previously admitted data.
    client, slack, calls = selected([
        page([row("x" * 2600)], response_metadata={"next_cursor": "later"}),
        httpx.Response(10 ** 1000),
    ])
    async with client:
        result = await slack.read_interval("C1", **DEFAULT, resumable=True, max_result_bytes=4096)
    assert_bounded(result, 4096)
    assert len(calls) == 2 and result["termination"] == "result_budget"
    assert len(result["messages"]) == 1 and result["history"]["pages"] == 1
    assert "error" not in result["history"]
    assert result["continuation"]["pending"][0]["cursor"] == "later"


async def test_unavailable_thread_and_request_budget_tails_remain_bounded():
    client, slack, calls = selected([
        page([{"ts": "11", "text": "x" * 1800}], response_metadata={"next_cursor": "later"}),
        page(ok=False, error="thread_not_found"),
    ])
    async with client:
        result = await slack.read_interval("C1", **{**DEFAULT, "max_requests": 2},
                                          resumable=True, max_result_bytes=4096)
    assert_bounded(result, 4096)
    assert len(calls) == 2 and result["termination"] == "request_budget"
    assert result["threads"][0]["stop_reason"] == "thread_not_found"
    assert [g["kind"] for g in result["coverage_gaps"]] == ["thread_not_found", "request_budget"]


async def test_default_splits_dense_week_and_resumes_every_exact_occurrence():
    pages = [page([row(str(i) + ":" + "x" * 48000, ts=str(10 + i / 10))],
                  response_metadata={"next_cursor": str(i + 1)}) for i in range(8)]
    pages[-1].pop("response_metadata")
    seen_cursors = []

    async def handler(request):
        cursor = request.url.params.get("cursor")
        seen_cursors.append(cursor)
        return httpx.Response(200, json=pages[int(cursor or 0)])

    from meerkat_toolkit.httpx_client import prepare_httpx_client
    from meerkat_toolkit.connectors.slack import SlackOperations
    async with httpx.AsyncClient(base_url="https://slack.invalid/", transport=httpx.MockTransport(handler)) as client:
        prepare_httpx_client(client)
        slack = SlackOperations(client)
        parts, checkpoint = [], None
        for _ in range(4):
            part = await slack.read_interval("C1", **DEFAULT, resumable=True, continuation=checkpoint)
            assert_bounded(part, DEFAULT_INTERVAL_RESULT_BYTES)
            parts.append(part)
            checkpoint = part["continuation"]
            if checkpoint is None:
                break
    assert len(parts) == 2 and parts[0]["termination"] == "result_budget"
    assert parts[-1]["termination"] == "finished" and checkpoint is None
    assert [r["message"] for p in parts for r in p["messages"]] == [p["messages"][0] for p in pages]
    assert seen_cursors == [None, "1", "2", "3", "4", "5", "5", "6", "7"]
