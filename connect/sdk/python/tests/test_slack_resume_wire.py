"""Real HTTP resume after a rate-limit response, using the existing request owner."""
from copy import deepcopy
from urllib.parse import parse_qs, urlsplit

import pytest

from meerkat_toolkit.connectors.slack import SlackOperations
from test_home_assistant import Server, client, respond


@pytest.mark.asyncio
async def test_loopback_resume_keeps_completed_pages_and_closes_every_handler():
    async def handle(request, writer):
        index = len(server.requests)
        if index == 1:
            payload = {"ok": True, "messages": [{"ts": "1"}],
                       "response_metadata": {"next_cursor": "history-next"}}
        elif index == 2:
            await respond(writer, {"ok": False, "error": "ratelimited"},
                          status=429, headers={"Retry-After": "60"})
            return
        elif index == 3:
            payload = {"ok": True, "messages": [{"ts": "11", "thread_ts": "1", "text": "weekly update"}]}
        else:
            assert index == 4
            payload = {"ok": True, "messages": []}
        await respond(writer, payload)

    args = dict(start="10", end="20", root_oldest="0", max_requests=10,
                max_messages=20, resumable=True)
    async with Server(handle) as server:
        async with client(server, token="synthetic-resume-token") as transport:
            slack = SlackOperations(transport)
            first = await slack.read_interval("C1", **args)
            assert first["termination"] == "request_failure"
            assert first["threads"][0]["error"]["retry_after_seconds"] == 60
            checkpoint = deepcopy(first["continuation"])
            assert [x["root_ts"] for x in checkpoint["pending"]] == ["1", None]
            # This explicit second call represents a caller-authorized later retry.
            second = await slack.read_interval("C1", **args, continuation=checkpoint)
            assert second["continuation"] is None
            assert [x["message"]["text"] for x in second["messages"]] == ["weekly update"]
            assert checkpoint == first["continuation"]
    assert server.joined and all(t.done() and t.exception() is None for t in server.tasks)
    assert len(server.requests) == 4
    targets = [urlsplit(request.target) for request in server.requests]
    assert [target.path.rsplit("/", 1)[-1] for target in targets] == [
        "conversations.history", "conversations.replies", "conversations.replies", "conversations.history"]
    assert parse_qs(targets[-1].query)["cursor"] == ["history-next"]
    assert all(request.headers["authorization"] == "Bearer synthetic-resume-token" for request in server.requests)
