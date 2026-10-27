"""Public bounded-interval controls using the existing page and response owners."""

import asyncio
import json

import httpx
import pytest

from meerkat_toolkit.connectors.slack import SlackOperations
from meerkat_toolkit.httpx_client import prepare_httpx_client


pytestmark = pytest.mark.asyncio
DEFAULT = dict(start="10", end="20", root_oldest="0", max_requests=10, max_messages=20)


def selected(pages, *, max_bytes=2_097_152, token="synthetic-one"):
    calls = []

    async def handle(request):
        calls.append(request)
        value = pages[len(calls) - 1]
        return value if isinstance(value, httpx.Response) else httpx.Response(200, json=value)

    client = prepare_httpx_client(httpx.AsyncClient(
        base_url="https://slack.invalid/proxy/", headers={"Authorization": token},
        transport=httpx.MockTransport(handle),
    ))
    return client, SlackOperations(client, max_response_bytes=max_bytes), calls


def page(messages=(), **metadata):
    return {"ok": True, "messages": list(messages), **metadata}


def kinds(result):
    return [gap["kind"] for gap in result["coverage_gaps"]]


async def test_public_interval_capability_exists():
    assert callable(getattr(SlackOperations, "read_interval", None))


async def test_old_roots_exact_rows_duplicates_and_fixed_bounds():
    first = {"ts": "15.000", "reply_count": None, "unknown": [None, False]}
    child = {"ts": "12.00", "thread_ts": "5.0", "text": "child", "unknown": {"n": None}}
    client, slack, calls = selected([
        page([{"ts": "5.0", "text": "older private body", "reply_count": 0}, first, {"ts": "20"}],
             has_more=True, response_metadata={"next_cursor": "opaque +/= ö"}),
        page([{"ts": "5.0", "text": "older private body"}, child, child,
              {"ts": "10", "thread_ts": "5.0"}, {"ts": "20", "thread_ts": "5.0"}]),
        page([first]), page(),
    ])
    async with client:
        result = await slack.read_interval("C1", **DEFAULT, page_limit=7, include_all_metadata=True)
        assert not client.is_closed
    assert [row["message"] for row in result["messages"]] == [first, child, child, {"ts": "10", "thread_ts": "5.0"}, first]
    assert "older private body" not in json.dumps(result)
    assert result["termination"] == "finished"
    assert result["history"]["state"] == "exhausted"
    assert [thread["root_ts"] for thread in result["threads"]] == ["5.0", "15.000"]
    assert result["coverage_gaps"] == []
    assert [request.url.path for request in calls] == [
        "/proxy/api/conversations.history", "/proxy/api/conversations.replies",
        "/proxy/api/conversations.replies", "/proxy/api/conversations.history",
    ]
    for index, request in enumerate(calls):
        assert request.url.params["channel"] == "C1"
        assert request.url.params["oldest"] == ("0" if index in (0, 3) else "10")
        assert request.url.params["latest"] == "20"
        assert request.url.params["inclusive"] == "true"
        assert request.url.params["limit"] == "7"
        assert request.url.params["include_all_metadata"] == "true"
        assert request.headers["authorization"] == "synthetic-one"
    assert calls[3].url.params["cursor"] == "opaque +/= ö"
    assert result["limits"] == dict(max_requests=10, max_messages=20, page_limit=7,
                                  max_response_bytes=2_097_152, max_result_bytes=262_144)


async def test_small_request_budget_makes_thread_progress_and_leaves_history_pending():
    child = {"ts": "11", "thread_ts": "1"}
    client, slack, calls = selected([
        page([{"ts": "1", "reply_count": 0}], response_metadata={"next_cursor": "history-next"}),
        page([child]),
    ])
    async with client:
        result = await slack.read_interval("C1", **{**DEFAULT, "max_requests": 2})
    assert [request.url.path.split("/")[-1] for request in calls] == ["conversations.history", "conversations.replies"]
    assert result["messages"] == [{"source": "replies", "root_ts": "1", "message": child}]
    assert result["termination"] == "request_budget"
    assert result["history"]["state"] == "pending"
    assert result["history"]["next_cursor"] == "history-next"
    assert result["threads"][0]["state"] == "exhausted"


async def test_projection_overflow_admits_no_rows_roots_gaps_or_progress_from_page():
    client, slack, calls = selected([
        page([], has_more=True, response_metadata={"next_cursor": "accepted"}),
        page([{"ts": "11"}, {"ts": "12"}, {"ts": "13"}], has_more=False,
             is_limited=True, response_metadata={"next_cursor": "refused"}),
    ])
    async with client:
        result = await slack.read_interval("C1", **{**DEFAULT, "max_messages": 2})
    assert len(calls) == 2
    assert result["messages"] == [] and result["threads"] == []
    assert result["history"] == {"state": "stopped", "pages": 1, "stop_reason": "message_budget", "next_cursor": "accepted", "has_more": True}
    assert result["termination"] == "message_budget"
    assert kinds(result) == ["message_budget"]


@pytest.mark.parametrize("metadata,reason", [({}, "missing_cursor"), ({"next_cursor": None}, "unusable_cursor"), ({"next_cursor": ""}, "unusable_cursor"), ({"next_cursor": "bad\n"}, "unusable_cursor")])
async def test_missing_or_unusable_cursor_is_not_exhaustion(metadata, reason):
    client, slack, calls = selected([page(has_more=True, response_metadata=metadata)])
    async with client:
        result = await slack.read_interval("C1", **DEFAULT)
    assert len(calls) == 1
    assert result["history"]["state"] == "stopped"
    assert result["history"]["stop_reason"] == reason
    assert result["termination"] == "finished"
    assert kinds(result) == [reason]
    assert ("next_cursor" in result["history"]) == ("next_cursor" in metadata)
    if "next_cursor" in metadata:
        assert result["history"]["next_cursor"] == metadata["next_cursor"]


@pytest.mark.parametrize("metadata", [{}, {"next_cursor": None}, {"next_cursor": ""}])
async def test_terminal_cursor_presence_is_preserved(metadata):
    client, slack, calls = selected([page(response_metadata=metadata)])
    async with client:
        result = await slack.read_interval("C1", **DEFAULT)
    assert len(calls) == 1 and result["history"]["state"] == "exhausted"
    assert ("next_cursor" in result["history"]) == ("next_cursor" in metadata)
    assert "has_more" not in result["history"]
    if "next_cursor" in metadata:
        assert result["history"]["next_cursor"] == metadata["next_cursor"]


async def test_fresh_cursor_overrides_false_hint_and_cycles_stop_without_time_fallback():
    client, slack, calls = selected([
        page(has_more=False, response_metadata={"next_cursor": "A"}),
        page(response_metadata={"next_cursor": "B"}),
        page(response_metadata={"next_cursor": "A"}),
    ])
    async with client:
        result = await slack.read_interval("C1", **DEFAULT)
    assert [request.url.params.get("cursor") for request in calls] == [None, "A", "B"]
    assert all(request.url.params["oldest"] == "0" for request in calls)
    assert result["history"]["stop_reason"] == "cursor_cycle"
    assert result["history"]["pages"] == 3
    assert kinds(result) == ["pagination_contradiction", "cursor_cycle"]


@pytest.mark.parametrize("marker,expected", [(True, "retention_limited"), (None, "invalid_coverage_metadata"), (0, "invalid_coverage_metadata"), ("true", "invalid_coverage_metadata")])
async def test_retention_metadata_is_observed_without_snapshot_claim(marker, expected):
    client, slack, _ = selected([page(is_limited=marker)])
    async with client:
        result = await slack.read_interval("C1", **{**DEFAULT, "root_oldest": "5"})
    assert kinds(result) == ["older_roots_unsearched", expected]
    assert result["termination"] == "finished" and "complete" not in result


@pytest.mark.parametrize("extra,reason", [
    ({"thread_ts": None}, "invalid_thread_reference"),
    ({"thread_ts": "13"}, "invalid_thread_reference"),
    ({"thread_ts": False}, "invalid_thread_reference"),
    ({"subtype": "thread_broadcast"}, "invalid_broadcast_reference"),
    ({"subtype": "thread_broadcast", "thread_ts": "1", "root": None}, "invalid_broadcast_reference"),
    ({"subtype": "reply_broadcast", "thread_ts": "1", "root": {"ts": "2"}}, "invalid_broadcast_reference"),
])
async def test_malformed_linkage_retains_selected_row_but_never_guesses_root(extra, reason):
    row = {"ts": "12", **extra}
    client, slack, calls = selected([page([row])])
    async with client:
        result = await slack.read_interval("C1", **DEFAULT)
    assert len(calls) == 1 and result["threads"] == []
    assert result["messages"] == [{"source": "history", "root_ts": None, "message": row}]
    assert kinds(result) == [reason]


async def test_broadcast_reference_can_cross_discovery_floor_without_auxiliary_body_dump():
    broadcast = {"ts": "12", "subtype": "thread_broadcast", "thread_ts": "1", "root": {"ts": "1", "text": "nested rendering"}}
    client, slack, calls = selected([page([broadcast]), page([{"ts": "13", "thread_ts": "1"}])])
    async with client:
        result = await slack.read_interval("C1", **{**DEFAULT, "root_oldest": "5"})
    assert calls[1].url.params["ts"] == "1"
    assert result["messages"][0]["message"] == broadcast
    assert kinds(result) == ["older_roots_unsearched"]


async def test_discovery_range_end_root_and_nonthreadable_skips_do_not_spend_queries():
    client, slack, calls = selected([page([
        {"ts": "4"}, {"ts": "21", "thread_ts": "1"}, {"ts": "20.00"},
        {"ts": "12", "subtype": "channel_join"}, {"ts": "13", "subtype": "channel_leave"},
    ])])
    async with client:
        result = await slack.read_interval("C1", **{**DEFAULT, "root_oldest": "5"})
    assert len(calls) == 1 and result["threads"] == []
    assert result["skipped_non_threadable"] == 2
    assert [row["message"]["ts"] for row in result["messages"]] == ["12", "13"]
    assert kinds(result) == ["older_roots_unsearched", "history_outside_discovery", "history_outside_discovery"]


async def test_reply_chain_mismatch_is_not_silently_assigned_as_observed_linkage():
    rows = [{"ts": "11", "thread_ts": "4"}, {"ts": "12"}]
    client, slack, _ = selected([page([{"ts": "1"}]), page(rows)])
    async with client:
        result = await slack.read_interval("C1", **DEFAULT)
    assert [row["message"] for row in result["messages"]] == rows
    assert all(row["root_ts"] == "1" for row in result["messages"])
    assert kinds(result) == ["reply_chain_mismatch", "reply_chain_mismatch"]


async def test_thread_not_found_continues_others_but_permission_stops_globally():
    client, slack, calls = selected([
        page([{"ts": "1"}, {"ts": "2"}, {"ts": "3"}]),
        {"ok": False, "error": "thread_not_found"},
        {"ok": False, "error": "missing_scope", "raw": "synthetic-private"},
    ])
    async with client:
        result = await slack.read_interval("C1", **DEFAULT)
    assert len(calls) == 3 and result["termination"] == "request_failure"
    assert [thread["state"] for thread in result["threads"]] == ["stopped", "stopped", "pending"]
    assert result["threads"][0]["error"] == dict(kind="not_found", status_code=200, provider_code="thread_not_found", retry_after_seconds=None)
    assert result["threads"][1]["error"]["provider_code"] == "missing_scope"
    assert "synthetic-private" not in json.dumps(result)


async def test_exact_decimals_beyond_float_precision_and_selected_connections():
    start, end = "9007199254740993.00000000000000001", "9007199254740993.00000000000000003"
    selected_row = {"ts": "9007199254740993.00000000000000002", "subtype": "channel_join", "false": False}
    for token in ("synthetic-A", "synthetic-B"):
        client, slack, calls = selected([page([{"ts": start, "subtype": "channel_join"}, selected_row, {"ts": end}])], token=token)
        async with client:
            result = await slack.read_interval("C1", start=start, end=end, root_oldest=start, max_requests=1, max_messages=3)
        assert len(result["messages"]) == 2 and result["messages"][1]["message"] == selected_row
        assert calls[0].url.params["oldest"] == start
        assert calls[0].headers["authorization"] == token


@pytest.mark.parametrize("changes", [
    {"start": "20"}, {"end": "10"}, {"root_oldest": "11"}, {"start": 10},
    {"max_requests": True}, {"max_requests": 0}, {"max_requests": 9007199254740992},
    {"max_messages": 0}, {"max_messages": 1.5}, {"page_limit": 1000}, {"include_all_metadata": 1},
])
async def test_invalid_local_selection_enters_no_request(changes):
    client, slack, calls = selected([])
    async with client:
        with pytest.raises(ValueError):
            await slack.read_interval("C1", **{**DEFAULT, **changes})
    assert calls == []


async def test_malformed_page_and_cap_are_partial_safe_failures_not_empty_exhaustion():
    for response, kind in [
        (httpx.Response(200, content=b'{"ok":true,"messages":[],"messages":[]}', headers={"Content-Type": "application/json"}), "invalid_response"),
        (httpx.Response(200, content=b"x" * 200, headers={"Content-Type": "application/json"}), "response_too_large"),
    ]:
        client, slack, calls = selected([response], max_bytes=100)
        async with client:
            result = await slack.read_interval("C1", **DEFAULT)
        assert len(calls) == 1 and result["messages"] == []
        assert result["history"]["pages"] == 0 and result["history"]["state"] == "stopped"
        assert result["history"]["error"]["kind"] == kind
        assert result["termination"] == "request_failure"


async def test_cancellation_propagates_only_after_owned_response_close():
    body_entered, closing, release, closed = (asyncio.Event() for _ in range(4))

    class Held(httpx.AsyncByteStream):
        async def __aiter__(self):
            body_entered.set()
            await asyncio.Event().wait()
            yield b""

        async def aclose(self):
            closing.set()
            await release.wait()
            closed.set()

    calls = []
    async def handle(request):
        calls.append(request)
        return httpx.Response(200, stream=Held(), headers={"Content-Type": "application/json"})

    async with prepare_httpx_client(httpx.AsyncClient(base_url="https://slack.invalid/", transport=httpx.MockTransport(handle))) as client:
        task = asyncio.create_task(SlackOperations(client).read_interval("C1", **DEFAULT))
        await asyncio.wait_for(body_entered.wait(), 2)
        task.cancel()
        await asyncio.wait_for(closing.wait(), 2)
        task.cancel()
        await asyncio.sleep(0)
        assert not task.done() and not closed.is_set()
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(task, 2)
        assert closed.is_set() and not client.is_closed and len(calls) == 1
