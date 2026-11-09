"""Public provider profile tests; synthetic signing/auth values only."""

import asyncio
from contextlib import asynccontextmanager
from dataclasses import FrozenInstanceError, asdict
import hashlib
import hmac
import importlib
import json

import httpx
import pytest

SECRET = b"synthetic-slack-signing-key"
NOW = 1_800_000_000


def api():
    return importlib.import_module("meerkat_toolkit.frontend.slack")


def event(**changes):
    value = {
        "type": "event_callback",
        "api_app_id": "A1",
        "team_id": "T1",
        "event_id": "Ev1",
        "event_time": NOW,
        "is_ext_shared_channel": False,
        "authorizations": [
            {"team_id": "T1", "user_id": "UBOT", "is_bot": True, "is_enterprise_install": False}
        ],
        "event": {
            "type": "message",
            "channel_type": "im",
            "channel": "D1",
            "user": "U1",
            "text": "Hej \U0001f43e <@U2>",
            "ts": "1800000000.000001",
            "event_ts": "1800000000.000002",
        },
    }
    value.update(changes)
    return value


def signed(value, timestamp=NOW):
    body = value if isinstance(value, bytes) else json.dumps(value, ensure_ascii=False).encode()
    stamp = str(timestamp)
    signature = (
        "v0=" + hmac.new(SECRET, b"v0:" + stamp.encode() + b":" + body, hashlib.sha256).hexdigest()
    )
    return body, [
        ("Content-Type", "application/json"),
        ("X-Slack-Signature", signature),
        ("X-Slack-Request-Timestamp", stamp),
    ]


def verify(value, *, timestamp=NOW, installation=None):
    m = api()
    return m.verify_event_request(
        *signed(value, timestamp),
        signing_secret=SECRET,
        installation=installation or m.SlackInstallation("A1", "T1", "UBOT"),
        now_seconds=NOW,
    )


def test_dm_and_thread_facts_are_exact_and_immutable():
    value = event()
    value["event"]["thread_ts"] = "000123.000456"
    result = verify(value)
    assert isinstance(result, api().SlackTextEvent)
    assert (result.app_id, result.team_id, result.event_id, result.user_id, result.channel_id) == (
        "A1",
        "T1",
        "Ev1",
        "U1",
        "D1",
    )
    assert (result.message_ts, result.thread_ts, result.event_ts) == (
        "1800000000.000001",
        "000123.000456",
        "1800000000.000002",
    )
    assert result.text == value["event"]["text"]
    assert result.body_sha256 == hashlib.sha256(signed(value)[0]).hexdigest()
    with pytest.raises(FrozenInstanceError):
        result.text = "changed"
    value["event"]["text"] = "changed"
    assert result.text.startswith("Hej")


def test_app_mention_keeps_text_and_missing_shared_is_not_private_evidence():
    value = event()
    value.pop("is_ext_shared_channel")
    value["event"].update(type="app_mention", channel="C1")
    value["event"].pop("channel_type")
    result = verify(value)
    assert result.event_type == "app_mention" and result.channel_id == "C1"
    assert result.thread_ts is None and result.text == value["event"]["text"]
    assert not hasattr(result, "authorized")


def test_signed_challenge_is_not_admission_or_installation_attestation():
    result = verify(
        {"type": "url_verification", "challenge": "exact-challenge", "token": "deprecated-canary"}
    )
    assert result.challenge == "exact-challenge"
    assert "deprecated-canary" not in repr(result)
    assert not hasattr(result, "team_id") and not hasattr(result, "admission_receipt")


@pytest.mark.parametrize(
    "kind",
    ["body", "secret", "signature", "duplicate_signature", "duplicate_timestamp", "duplicate_type"],
)
def test_signature_and_duplicate_header_refusals(kind):
    m = api()
    body, headers = signed(event())
    secret = SECRET
    if kind == "body":
        body += b" "
    if kind == "secret":
        secret = b"wrong-synthetic-secret"
    if kind == "signature":
        headers[1] = ("x-slack-signature", "v0=" + "0" * 64)
    if kind.startswith("duplicate_"):
        headers.append(
            headers[{"duplicate_signature": 1, "duplicate_timestamp": 2, "duplicate_type": 0}[kind]]
        )
    with pytest.raises(m.SlackFrontendError) as caught:
        m.verify_event_request(
            body,
            headers,
            signing_secret=secret,
            installation=m.SlackInstallation("A1", "T1", "UBOT"),
            now_seconds=NOW,
        )
    assert caught.value.__cause__ is None and caught.value.__context__ is None


@pytest.mark.parametrize("offset", [-301, 301])
def test_stale_and_future_refuse(offset):
    with pytest.raises(api().SlackFrontendError):
        verify(event(), timestamp=NOW + offset)


@pytest.mark.parametrize("offset", [-300, 300])
def test_freshness_boundary_is_inclusive(offset):
    assert verify(event(), timestamp=NOW + offset).event_id == "Ev1"


@pytest.mark.parametrize(
    "field,value", [("api_app_id", "A2"), ("team_id", "T2"), ("context_team_id", "T2")]
)
def test_wrong_installation_refuses(field, value):
    with pytest.raises(api().SlackFrontendError):
        verify(event(**{field: value}))


@pytest.mark.parametrize(
    "body",
    [
        b'{"type":"url_verification","challenge":"one","challenge":"two"}',
        b'{"type":"url_verification","challenge":NaN}',
        b"\xff",
        b"[]",
    ],
)
def test_invalid_json_is_bounded_and_has_no_raw_cause(body):
    with pytest.raises(api().SlackFrontendError) as caught:
        verify(body)
    assert caught.value.__cause__ is None and caught.value.__context__ is None
    assert len(str(caught.value)) < 150


@pytest.mark.parametrize(
    "mutation",
    [{"user": True}, {"text": None}, {"ts": 1.2}, {"text": "\ud800"}, {"thread_ts": False}],
)
def test_malformed_supported_event_refuses(mutation):
    value = event()
    value["event"].update(mutation)
    body = json.dumps(value).encode()
    with pytest.raises(api().SlackFrontendError):
        verify(body)


@pytest.mark.parametrize(
    "mutation",
    [
        {"subtype": "message_changed"},
        {"edited": {}},
        {"bot_id": "B1"},
        {"user": "UBOT"},
        {"files": [{"id": "F1"}]},
        {"attachments": [{}]},
        {"channel_type": "mpim"},
    ],
)
def test_unsupported_message_profiles_are_explicit(mutation):
    value = event()
    value["event"].update(mutation)
    assert isinstance(verify(value), api().SlackUnsupportedEvent)


@pytest.mark.parametrize(
    "field,value", [("is_ext_shared_channel", True), ("context_enterprise_id", "E1")]
)
def test_enterprise_and_shared_are_explicitly_unsupported(field, value):
    assert isinstance(verify(event(**{field: value})), api().SlackUnsupportedEvent)


def test_existing_inbox_owns_replay_and_separate_users_threads(tmp_path):
    from meerkat_toolkit.frontend.inbox import SQLiteChannelInbox

    m = api()
    first = verify(event())
    retry = verify(event(), timestamp=NOW + 1)
    assert first == retry
    inbox = SQLiteChannelInbox(path=tmp_path / "inbox.sqlite")
    for facts in (first, retry):
        inbox._insert(
            channel_id="host-selected-installation-A1-T1",
            source_event_id=facts.event_id,
            state="accepted",
            envelope=asdict(facts),
            dispatch_key="host-test-key",
            reject_reason=None,
        )
    assert inbox.event_count("host-selected-installation-A1-T1", "Ev1") == 1
    other = event(event_id="Ev2")
    other["event"].update(user="U2", thread_ts="001.02")
    facts = verify(other)
    assert (facts.user_id, facts.thread_ts) != (first.user_id, first.thread_ts)
    assert m.SlackInstallation("A1", "T1", "UBOT").team_id == "T1"
    # This fixture supplies host custody explicitly; the verifier did not write it.
    assert inbox.event_count("other-host-installation", "Ev1") == 0


def prepared(**kwargs):
    from meerkat_toolkit.httpx_client import prepare_httpx_client

    return prepare_httpx_client(httpx.AsyncClient(trust_env=False, **kwargs))


def success(*, thread="1800000000.000001", channel="D1"):
    message = {"type": "message", "ts": "1800000001.000001"}
    if thread is not None:
        message["thread_ts"] = thread
    return {"ok": True, "channel": channel, "ts": "1800000001.000001", "message": message}


@pytest.mark.asyncio
async def test_reply_exact_wire_scope_and_borrowed_lifetime():
    m = api()
    seen = []

    async def handler(request):
        seen.append(request)
        return httpx.Response(200, json=success())

    async with prepared(
        base_url="https://slack.invalid/proxy/",
        headers={
            "Authorization": "Bearer synthetic",
            "If-Match": "ambient",
            "Range": "bytes=0-4",
            "X-HTTP-Method-Override": "DELETE",
        },
        params={"channel": "other"},
        transport=httpx.MockTransport(handler),
    ) as client:
        transport = m.SlackReplyTransport(client)
        result = await transport.send_text(
            "D1", "Exact \U0001f43e <@U2>", thread_ts="1800000000.000001"
        )
        assert not client.is_closed
    assert (
        len(seen) == 1
        and seen[0].url.path == "/proxy/api/chat.postMessage"
        and not seen[0].url.query
    )
    wire = json.loads(seen[0].content)
    assert wire == {
        "channel": "D1",
        "text": "Exact \U0001f43e <@U2>",
        "thread_ts": "1800000000.000001",
        "reply_broadcast": False,
        "unfurl_links": False,
        "unfurl_media": False,
        "unfurl_app_links": False,
        "mrkdwn": False,
        "parse": "none",
        "link_names": False,
    }
    assert seen[0].headers["authorization"] == "Bearer synthetic"
    assert not any(k in seen[0].headers for k in ("if-match", "range", "x-http-method-override"))
    assert result.outcome == "sent" and result.provider_message_id == "1800000001.000001"
    assert result.observed_thread_ts == result.requested_thread_ts == "1800000000.000001"


@pytest.mark.asyncio
@pytest.mark.parametrize("change", [{"channel": "D2"}, {"thread": "1.1"}])
async def test_contradictory_receipt_unknown_without_retry(change):
    calls = []

    async def handler(request):
        calls.append(request)
        return httpx.Response(200, json=success(**change))

    async with prepared(
        base_url="https://slack.invalid/", transport=httpx.MockTransport(handler)
    ) as client:
        result = (
            await api()
            .SlackReplyTransport(client)
            .send_text("D1", "hello", thread_ts="1800000000.000001")
        )
    assert result.outcome == "unknown" and len(calls) == 1


@pytest.mark.asyncio
async def test_missing_returned_thread_does_not_invent_observation():
    async with prepared(
        base_url="https://slack.invalid/",
        transport=httpx.MockTransport(
            lambda request: httpx.Response(200, json=success(thread=None))
        ),
    ) as client:
        result = (
            await api()
            .SlackReplyTransport(client)
            .send_text("D1", "hello", thread_ts="1800000000.000001")
        )
    assert result.outcome == "sent" and result.observed_thread_ts is None
    assert result.requested_thread_ts == "1800000000.000001"


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "body",
    [
        b'{"ok":true,"ok":false}',
        b'{"ok":true}',
        b'{"ok":false,"error":"internal_error"}',
        b'{"ok":false,"error":"secret-canary"}',
    ],
)
async def test_malformed_or_ambiguous_response_unknown_and_safe(body):
    calls = []

    async def handler(request):
        calls.append(request)
        return httpx.Response(200, content=body, headers={"content-type": "application/json"})

    async with prepared(
        base_url="https://slack.invalid/", transport=httpx.MockTransport(handler)
    ) as client:
        result = await api().SlackReplyTransport(client).send_text("D1", "hello", thread_ts="1.1")
    assert result.outcome == "unknown" and len(calls) == 1 and "secret-canary" not in repr(result)


@pytest.mark.asyncio
async def test_known_explicit_refusal_has_no_adapter_retry():
    calls = []

    async def handler(request):
        calls.append(request)
        return httpx.Response(200, json={"ok": False, "error": "invalid_auth"})

    async with prepared(
        base_url="https://slack.invalid/", transport=httpx.MockTransport(handler)
    ) as client:
        result = await api().SlackReplyTransport(client).send_text("D1", "hello", thread_ts="1.1")
    assert result.outcome == "known_not_sent" and len(calls) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "body,headers",
    [
        (b'{"ok":true}', {"content-length": "99"}),
        (b'{"ok":true}', {"content-length": "x"}),
        (b"x" * 200, {}),
        (b"{}", {"content-encoding": "gzip"}),
    ],
)
async def test_stream_profile_failures_are_unknown(body, headers):
    headers = {"content-type": "application/json", **headers}
    async with prepared(
        base_url="https://slack.invalid/",
        transport=httpx.MockTransport(
            lambda request: httpx.Response(200, content=body, headers=headers)
        ),
    ) as client:
        result = (
            await api()
            .SlackReplyTransport(client, max_response_bytes=100)
            .send_text("D1", "hello", thread_ts="1.1")
        )
    assert result.outcome == "unknown"


@pytest.mark.asyncio
async def test_local_unicode_and_unprepared_refuse_before_auth_or_request():
    calls = []

    async def handler(request):
        calls.append(request)
        return httpx.Response(200, json=success())

    async with httpx.AsyncClient(
        base_url="https://slack.invalid/", transport=httpx.MockTransport(handler), trust_env=False
    ) as client:
        transport = api().SlackReplyTransport(client)
        for text in ("normal", "\ud800"):
            with pytest.raises(api().SlackFrontendError) as caught:
                await transport.send_text("D1", text, thread_ts="1.1")
            assert caught.value.write_outcome == "not_attempted"
            assert caught.value.__cause__ is None and caught.value.__context__ is None
    assert calls == []


@pytest.mark.asyncio
async def test_arbitrary_host_exception_is_sanitized():
    async def handler(request):
        raise RuntimeError("synthetic-secret-canary")

    async with prepared(
        base_url="https://slack.invalid/", transport=httpx.MockTransport(handler)
    ) as client:
        result = await api().SlackReplyTransport(client).send_text("D1", "hello", thread_ts="1.1")
    assert result.outcome == "unknown" and "synthetic-secret-canary" not in repr(result)


class HeldClose(httpx.AsyncByteStream):
    def __init__(self):
        self.entered = asyncio.Event()
        self.release = asyncio.Event()
        self.done = False

    async def __aiter__(self):
        yield json.dumps(success()).encode()

    async def aclose(self):
        self.entered.set()
        await self.release.wait()
        self.done = True


@pytest.mark.asyncio
@pytest.mark.parametrize("cancel", [True, False])
async def test_eof_cleanup_is_joined_on_cancel_or_deadline(cancel):
    stream = HeldClose()
    async with prepared(
        base_url="https://slack.invalid/",
        transport=httpx.MockTransport(
            lambda request: httpx.Response(
                200, stream=stream, headers={"content-type": "application/json"}
            )
        ),
    ) as client:
        task = asyncio.create_task(
            api()
            .SlackReplyTransport(client, timeout_seconds=0.03 if not cancel else 5)
            .send_text("D1", "hello", thread_ts="1800000000.000001")
        )
        await asyncio.wait_for(stream.entered.wait(), 1)
        if cancel:
            task.cancel()
            await asyncio.sleep(0)
            task.cancel()
        await asyncio.sleep(0.06)
        assert not task.done()
        stream.release.set()
        if cancel:
            with pytest.raises(asyncio.CancelledError):
                await task
        else:
            assert (await task).outcome == "unknown"
        assert stream.done and not client.is_closed


@asynccontextmanager
async def drop_after_post():
    requests = []
    tasks = set()

    async def handler(reader, writer):
        task = asyncio.current_task()
        tasks.add(task)
        try:
            headers = await reader.readuntil(b"\r\n\r\n")
            size = next(
                int(row.split(b":", 1)[1])
                for row in headers.split(b"\r\n")
                if row.lower().startswith(b"content-length:")
            )
            requests.append(await reader.readexactly(size))
        finally:
            writer.close()
            await writer.wait_closed()
            tasks.discard(task)

    server = await asyncio.start_server(handler, "127.0.0.1", 0)
    try:
        yield f"http://127.0.0.1:{server.sockets[0].getsockname()[1]}/", requests
    finally:
        server.close()
        await server.wait_closed()
        if tasks:
            await asyncio.gather(*tuple(tasks))


@pytest.mark.asyncio
async def test_real_loopback_lost_reply_remains_unknown_in_existing_egress(tmp_path):
    from meerkat_toolkit.frontend.egress import ProactiveEgress

    async with drop_after_post() as (base, requests):
        async with prepared(base_url=base, headers={"authorization": "Bearer synthetic"}) as client:
            transport = api().SlackReplyTransport(client)

            async def sender(**kwargs):
                assert kwargs["recipient"] == "host-bound-D1-thread1"
                return await transport.send_text("D1", kwargs["part"]["text"], thread_ts="1.1")

            kwargs = {
                "path": tmp_path / "egress.sqlite",
                "senders": {"slack": sender},
                "incident_sink": lambda row: None,
                "health_sink": lambda row: None,
                "sleep": asyncio.sleep,
            }
            owner = ProactiveEgress(**kwargs)
            output = owner.commit_output(
                issuer_id="host-principal",
                logical_key="reply-Ev1",
                candidates=[("slack", "host-bound-D1-thread1")],
                parts=[{"text": "hello"}],
                incident_delivery=False,
            )
            await owner._dispatch_output(output)
            assert owner.status(output_id=output).delivery_state == "unknown"
            reopened = ProactiveEgress(**kwargs)
            await reopened.recover_pending()
            assert reopened.attempt_count(output_id=output) == 1
    assert len(requests) == 1 and json.loads(requests[0])["thread_ts"] == "1.1"
