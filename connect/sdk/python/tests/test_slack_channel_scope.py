"""Fixed channel routing ceilings through the real connector/request owner."""
from copy import deepcopy

import httpx
import pytest

from meerkat_toolkit.connectors.slack import FailureKind, SlackError, SlackOperations
from meerkat_toolkit.httpx_client import prepare_httpx_client


DEFAULT = dict(start="10", end="20", root_oldest="0", max_requests=1,
               max_messages=20, resumable=True)
OMITTED = object()


class CountingAuth(httpx.Auth):
    def __init__(self, account):
        self.account, self.calls = account, 0

    async def async_auth_flow(self, request):
        self.calls += 1
        request.headers["Authorization"] = "Bearer fixture-" + self.account
        yield request


def selected(scope=OMITTED, *, account="one", response=None):
    requests, auth = [], CountingAuth(account)

    def handle(request):
        requests.append(request)
        if response is not None:
            value = response(request)
        elif request.url.path.endswith("auth.test"):
            value = {"ok": True, "team_id": "T1", "user_id": "U1"}
        elif request.url.path.endswith("conversations.list"):
            value = {"ok": True, "channels": [{"id": "C2", "name": "other"}],
                     "response_metadata": {"next_cursor": "all-channels"}}
        elif request.url.path.endswith("conversations.info"):
            value = {"ok": True, "channel": {"id": request.url.params["channel"]}}
        elif request.url.path.endswith("chat.getPermalink"):
            value = {"ok": True, "channel": request.url.params["channel"],
                     "permalink": "https://workspace.invalid/message"}
        else:
            value = {"ok": True, "messages": []}
        return httpx.Response(200, json=value)

    client = prepare_httpx_client(httpx.AsyncClient(
        base_url="https://slack.invalid/", auth=auth,
        transport=httpx.MockTransport(handle), trust_env=False))
    kwargs = {} if scope is OMITTED else {"allowed_channel_ids": scope}
    return client, SlackOperations(client, **kwargs), requests, auth


def permission(error):
    assert error.kind is FailureKind.PERMISSION
    assert str(error) == "Slack permission"
    assert error.status_code is None and error.provider_code is None
    assert error.retry_after_seconds is None
    assert error.__cause__ is None and error.__context__ is None


@pytest.mark.asyncio
@pytest.mark.parametrize("scope", [[], ["C1"]])
async def test_all_channel_calls_and_broad_list_refuse_before_auth(scope):
    client, slack, requests, auth = selected(scope)
    async with client:
        calls = [lambda: slack.history_page("C2"), lambda: slack.replies_page("C2", "1"),
                 lambda: slack.get_conversation("C2"), lambda: slack.get_permalink("C2", "1"),
                 lambda: slack.read_interval("C2", **DEFAULT),
                 lambda: slack.list_conversations_page(),
                 lambda: slack.list_conversations_page(cursor="previous-unrestricted-cursor")]
        for call in calls:
            with pytest.raises(SlackError) as raised:
                await call()
            permission(raised.value)
        assert requests == [] and auth.calls == 0
        assert (await slack.auth_test())["team_id"] == "T1"
        assert len(requests) == auth.calls == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("scope", [OMITTED, None])
async def test_absent_and_none_preserve_unrestricted_behavior(scope):
    client, slack, requests, auth = selected(scope)
    async with client:
        assert (await slack.get_conversation("C2"))["channel"]["id"] == "C2"
        assert (await slack.get_permalink("C2", "1"))["channel"] == "C2"
        assert (await slack.history_page("C2"))["messages"] == []
        assert (await slack.replies_page("C2", "1"))["messages"] == []
        legacy = await slack.read_interval("C2", **{**DEFAULT, "resumable": False})
        assert "continuation" not in legacy and "observation_scope" not in legacy
        assert (await slack.list_conversations_page())["response_metadata"]["next_cursor"] == "all-channels"
        assert len(requests) == auth.calls == 6


@pytest.mark.asyncio
async def test_constructor_copies_membership_and_accounts_remain_separate():
    allowed = ["C1"]
    first, one, requests_one, auth_one = selected(allowed, account="one")
    second, two, requests_two, auth_two = selected(("C2",), account="two")
    allowed[:] = ["C2"]
    async with first, second:
        await one.history_page("C1")
        await two.history_page("C2")
        for operation, forbidden in ((one, "C2"), (two, "C1"), (one, "c1")):
            with pytest.raises(SlackError) as raised:
                await operation.history_page(forbidden)
            permission(raised.value)
    assert len(requests_one) == len(requests_two) == auth_one.calls == auth_two.calls == 1
    assert requests_one[0].headers["Authorization"] == "Bearer fixture-one"
    assert requests_two[0].headers["Authorization"] == "Bearer fixture-two"


@pytest.mark.asyncio
async def test_resume_rechecks_current_ceiling_and_preserves_successful_parts():
    def response(request):
        rows = ([{"ts": "1"}] if request.url.path.endswith("conversations.history") else
                [{"ts": "11", "thread_ts": "1", "text": "A quote mentioning C2 is preserved"}])
        return {"ok": True, "messages": rows}

    old_client, old, old_requests, _ = selected(response=response)
    client, narrowed, requests, auth = selected(["C1"], response=response)
    async with old_client, client:
        previous = await old.read_interval("C2", **DEFAULT)
        checkpoint = deepcopy(previous["continuation"])
        with pytest.raises(SlackError) as raised:
            await narrowed.read_interval("C2", **DEFAULT, continuation=checkpoint)
        permission(raised.value)
        with pytest.raises(ValueError):
            await narrowed.read_interval("C1", **DEFAULT, continuation=checkpoint)
        assert requests == [] and auth.calls == 0
        assert checkpoint == previous["continuation"]
        first = await narrowed.read_interval("C1", **DEFAULT)
        last = await narrowed.read_interval("C1", **DEFAULT, continuation=first["continuation"])
        assert first["termination"] == "request_budget"
        assert last["continuation"] is None and last["history"]["state"] == "not_requested"
        assert last["messages"][0]["message"]["text"] == "A quote mentioning C2 is preserved"
        assert [r.url.params["channel"] for r in requests] == ["C1", "C1"]
        assert len(old_requests) == 1 and auth.calls == 2


@pytest.mark.asyncio
async def test_strict_bounded_constructor_input():
    invalid = [True, "C1", {"C1"}, {}, [None], [True], [1], [""], ["C1\n"],
               ["C-1"], ["C" * 129], ["C1", "C1"], [f"C{i}" for i in range(257)]]
    client, _, requests, auth = selected()
    async with client:
        for value in invalid:
            with pytest.raises(ValueError, match="allowed_channel_ids"):
                SlackOperations(client, allowed_channel_ids=value)
        limit = SlackOperations(client, allowed_channel_ids=[f"C{i}" for i in range(256)])
        await limit.history_page("C255")
        exact = SlackOperations(client, allowed_channel_ids=("C" * 128, "c1", "C1"))
        await exact.history_page("C" * 128)
    assert len(requests) == auth.calls == 2


@pytest.mark.asyncio
@pytest.mark.parametrize("operation", ["info", "permalink"])
async def test_recognized_response_identity_mismatch_never_returns_foreign_data(operation):
    value = ({"ok": True, "channel": {"id": "C2", "name": "foreign-secret"}} if operation == "info" else
             {"ok": True, "channel": "C2", "permalink": "https://foreign.invalid/secret"})
    client, slack, requests, auth = selected(["C1"], response=lambda _: value)
    async with client:
        with pytest.raises(SlackError) as raised:
            if operation == "info":
                await slack.get_conversation("C1")
            else:
                await slack.get_permalink("C1", "1")
    assert raised.value.kind is FailureKind.INVALID_RESPONSE
    assert "foreign" not in str(raised.value) and "C2" not in str(raised.value)
    assert len(requests) == auth.calls == 1
