"""Provider permalink retrieval through the existing borrowed-client owner."""

import asyncio
import traceback

import httpx
import pytest

from meerkat_toolkit.connectors.slack import FailureKind, SlackError, SlackOperations
from meerkat_toolkit.httpx_client import prepare_httpx_client


LINK = "https://northstar.slack.com/archives/C123/p135854651700023?thread_ts=1358546515.000008&cid=C123"
PAYLOAD = {"ok": True, "channel": "C123", "permalink": LINK, "extra": {"keep": None}}


def client(handler):
    value = httpx.AsyncClient(base_url="https://slack.com/", transport=httpx.MockTransport(handler),
                             headers={"Authorization": "Bearer synthetic-A"},
                             params={"channel": "UNSELECTED", "token": "unselected"},
                             trust_env=False, follow_redirects=False)
    prepare_httpx_client(value)
    return value


async def test_permalink_preserves_provider_link_and_exact_selected_request():
    requests = []

    async def handle(request):
        requests.append(request)
        return httpx.Response(200, json=PAYLOAD)

    async with client(handle) as transport:
        result = await SlackOperations(transport).get_permalink("C123", "1358546517.000023")
        assert result == PAYLOAD
        assert not transport.is_closed
    assert len(requests) == 1
    request = requests[0]
    assert request.method == "GET"
    assert str(request.url.copy_with(query=None)) == "https://slack.com/api/chat.getPermalink"
    assert dict(request.url.params) == {"channel": "C123", "message_ts": "1358546517.000023"}
    assert request.headers["authorization"] == "Bearer synthetic-A"
    assert request.content == b""


@pytest.mark.parametrize("channel,message_ts", [
    ("C1/other", "1.0"), ("C1\n", "1.0"), ("", "1.0"), (True, "1.0"),
    ("C123", 1.0), ("C123", "1.0&token=x"), ("C123", "1.0\n"),
    ("C123", ""), ("C123", "1" * 65), ("C123", "-1.0"),
])
async def test_permalink_invalid_selection_never_enters_provider(channel, message_ts):
    requests = []

    async def handle(request):
        requests.append(request)
        return httpx.Response(200, json=PAYLOAD)

    async with client(handle) as transport:
        with pytest.raises(ValueError):
            await SlackOperations(transport).get_permalink(channel, message_ts)
    assert requests == []


@pytest.mark.parametrize("changes", [
    {"channel": "COTHER"}, {"channel": None}, {"permalink": None}, {"permalink": ""},
    {"permalink": "javascript:alert(1)"}, {"permalink": "http://northstar.slack.com/a"},
    {"permalink": "https://user:secret@northstar.slack.com/a"},
    {"permalink": "https://northstar.slack.com/a\n"},
    {"permalink": "https://northstar.slack.com/" + "x" * 8192},
    {"permalink": "https://[invalid-address]/secret"},
    {"permalink": "https://northstar.slack.com:invalid-port/secret"},
])
async def test_permalink_malformed_response_is_safe_failure(changes):
    async def handle(_request):
        return httpx.Response(200, json={**PAYLOAD, **changes})

    async with client(handle) as transport:
        with pytest.raises(SlackError) as caught:
            await SlackOperations(transport).get_permalink("C123", "1358546517.000023")
    assert caught.value.kind is FailureKind.INVALID_RESPONSE
    assert "secret" not in "".join(traceback.format_exception(caught.value))


@pytest.mark.parametrize("status,payload,kind", [
    (200, {"ok": False, "error": "channel_not_found"}, FailureKind.NOT_FOUND),
    (200, {"ok": False, "error": "message_not_found"}, FailureKind.NOT_FOUND),
    (200, {"ok": False, "error": "missing_scope"}, FailureKind.PERMISSION),
    (200, {"ok": False, "error": "secret-private-provider-detail"}, FailureKind.PROVIDER),
    (429, {}, FailureKind.RATE_LIMITED),
    (302, {}, FailureKind.REDIRECT),
])
async def test_permalink_failures_never_retry_or_follow_links(status, payload, kind):
    requests = []

    async def handle(request):
        requests.append(request)
        return httpx.Response(status, json=payload,
                              headers={"Retry-After": "7", "Location": "https://unselected.invalid/"})

    async with client(handle) as transport:
        with pytest.raises(SlackError) as caught:
            await SlackOperations(transport).get_permalink("C123", "1358546517.000023")
    assert caught.value.kind is kind
    assert len(requests) == 1
    assert "secret-private-provider-detail" not in "".join(traceback.format_exception(caught.value))


async def test_permalink_cancellation_joins_response_close():
    entered, release, closed = asyncio.Event(), asyncio.Event(), asyncio.Event()

    class Stream(httpx.AsyncByteStream):
        async def __aiter__(self):
            entered.set()
            await asyncio.Event().wait()
            yield b""

        async def aclose(self):
            await release.wait()
            closed.set()

    async def handle(_request):
        return httpx.Response(200, headers={"Content-Type": "application/json"}, stream=Stream())

    async with client(handle) as transport:
        call = asyncio.create_task(SlackOperations(transport).get_permalink("C123", "1.0"))
        await asyncio.wait_for(entered.wait(), 2)
        call.cancel()
        await asyncio.sleep(0)
        call.cancel()
        assert not call.done()
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(call, 2)
        assert closed.is_set()
