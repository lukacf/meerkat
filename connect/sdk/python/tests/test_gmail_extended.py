"""Public Gmail operations through actual HTTP or controlled close ownership."""

from __future__ import annotations

import asyncio
import base64
import json
import traceback

import anyio
import httpx
from meerkat_toolkit.httpx_client import prepare_httpx_client
import pytest

from meerkat_toolkit.connectors.gmail import (
    FailureKind,
    GmailError,
    GmailOperations,
    SendCancelled,
    WriteOutcome,
)
from test_gmail import MIME, RECEIPT
from test_home_assistant import Server, client, respond


@pytest.mark.asyncio
async def test_new_operations_use_exact_receipts_and_message_bytes():
    payload = b"attachment\x00bytes"
    attachment = {
        "size": len(payload),
        "data": base64.urlsafe_b64encode(payload).decode(),
        "extra": 3,
    }
    draft = {"id": "draft_1", "message": RECEIPT, "future": True}
    replies = iter([RECEIPT, draft, RECEIPT, attachment, {"size": 0, "data": ""}])

    async def handler(request, writer):
        await respond(writer, next(replies))

    async with Server(handler) as server:
        async with client(server) as transport:
            ops = GmailOperations(transport, user_id="me")
            assert await ops.send_raw(MIME, thread_id="thread_01") == RECEIPT
            assert await ops.create_draft_raw(MIME, thread_id="thread_01") == draft
            assert (
                await ops.modify_message("sent_01", remove_label_ids=["INBOX", "UNREAD"]) == RECEIPT
            )
            assert await ops.get_attachment("sent_01", "att_1") == attachment
            assert await ops.get_attachment("sent_01", "empty") == {"size": 0, "data": ""}
        assert [r.target for r in server.requests] == [
            "/gmail/v1/users/me/messages/send",
            "/gmail/v1/users/me/drafts",
            "/gmail/v1/users/me/messages/sent_01/modify",
            "/gmail/v1/users/me/messages/sent_01/attachments/att_1",
            "/gmail/v1/users/me/messages/sent_01/attachments/empty",
        ]
        send, draft_request, modify = [json.loads(r.body) for r in server.requests[:3]]
        assert base64.urlsafe_b64decode(send["raw"]) == MIME
        assert draft_request == {"message": send}
        assert send["threadId"] == "thread_01"
        assert modify == {"removeLabelIds": ["INBOX", "UNREAD"]}
    assert server.joined


@pytest.mark.asyncio
@pytest.mark.parametrize("operation", ["send", "draft", "modify"])
@pytest.mark.parametrize("status", [401, 403, 429, 500])
async def test_every_write_failure_is_unknown_and_not_replayed(operation, status):
    async def handler(request, writer):
        await respond(writer, {"error": {"message": "private auth failure canary"}}, status=status)

    async with Server(handler) as server:
        async with client(server) as transport:
            ops = GmailOperations(transport, user_id="me")
            with pytest.raises(GmailError) as caught:
                await {
                    "send": lambda: ops.send_raw(MIME),
                    "draft": lambda: ops.create_draft_raw(MIME),
                    "modify": lambda: ops.modify_message("sent_01", add_label_ids=["INBOX"]),
                }[operation]()
            assert caught.value.write_outcome is WriteOutcome.UNKNOWN
            assert caught.value.status_code == status
            assert "canary" not in "".join(traceback.format_exception(caught.value))
        assert len(server.requests) == 1
    assert server.joined


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "operation,receipt",
    [
        ("draft", {}),
        ("draft", {"id": "draft"}),
        ("draft", {"id": "draft", "message": {"id": "message"}}),
        ("modify", {"id": "wrong", "threadId": "thread"}),
        ("modify", {"id": "sent_01"}),
        ("attachment", {"data": "YQ"}),
        ("attachment", {"size": True, "data": "YQ"}),
        ("attachment", {"size": 2, "data": "YQ"}),
        ("attachment", {"size": 1, "data": "a?"}),
    ],
)
async def test_incomplete_or_mismatched_receipts_refuse(operation, receipt):
    async def handler(request, writer):
        await respond(writer, receipt)

    async with Server(handler) as server:
        async with client(server) as transport:
            ops = GmailOperations(transport, user_id="me")
            with pytest.raises(GmailError) as caught:
                await {
                    "draft": lambda: ops.create_draft_raw(MIME),
                    "modify": lambda: ops.modify_message("sent_01", remove_label_ids=["INBOX"]),
                    "attachment": lambda: ops.get_attachment("sent_01", "att"),
                }[operation]()
            assert caught.value.kind is FailureKind.INVALID_RESPONSE
            assert caught.value.write_outcome is (
                WriteOutcome.NOT_APPLICABLE if operation == "attachment" else WriteOutcome.UNKNOWN
            )
        assert len(server.requests) == 1
    assert server.joined


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "raw", [b'{"id":"draft","id":"other","message":{}}', b'{"id":"draft","message":{},"x":1e999}']
)
async def test_new_write_strict_json_does_not_accept_receipt(raw):
    async def handler(request, writer):
        await respond(writer, {}, raw=raw)

    async with Server(handler) as server:
        async with client(server) as transport:
            with pytest.raises(GmailError) as caught:
                await GmailOperations(transport, user_id="me").create_draft_raw(MIME)
            assert caught.value.kind is FailureKind.INVALID_RESPONSE
            assert caught.value.write_outcome is WriteOutcome.UNKNOWN
        assert len(server.requests) == 1


@pytest.mark.asyncio
async def test_explicit_auth_and_base_do_not_mutate_shared_defaults_or_retarget():
    class Auth(httpx.Auth):
        def auth_flow(self, request):
            request.headers["Authorization"] = "Bearer selected"
            yield request

    async def handler(request, writer):
        assert request.headers["authorization"] == "Bearer selected"
        assert "x-http-method-override" not in request.headers
        await respond(writer, RECEIPT)

    async with Server(handler) as server:
        async with prepare_httpx_client(httpx.AsyncClient(
            base_url="https://unused.invalid/",
            trust_env=False,
            headers={"Authorization": "Bearer ambient", "X-HTTP-Method-Override": "DELETE"},
            params={"fields": "id"},
        )) as transport:
            await GmailOperations(
                transport, user_id="selected@example.test", base_url=server.url, auth=Auth()
            ).send_raw(MIME)
            assert transport.headers["Authorization"] == "Bearer ambient"
            assert str(transport.base_url) == "https://unused.invalid/"
        assert server.requests[0].target == "/gmail/v1/users/selected%40example.test/messages/send"
    assert server.joined


@pytest.mark.asyncio
async def test_labels_snapshot_before_async_auth_suspension():
    entered, release = asyncio.Event(), asyncio.Event()

    class Auth(httpx.Auth):
        async def async_auth_flow(self, request):
            entered.set()
            await release.wait()
            yield request

    async def handler(request, writer):
        assert json.loads(request.body) == {"addLabelIds": ["INBOX"]}
        await respond(writer, RECEIPT)

    async with Server(handler) as server:
        async with client(server) as transport:
            labels = ["INBOX"]
            task = asyncio.create_task(
                GmailOperations(transport, user_id="me", auth=Auth()).modify_message(
                    "sent_01", add_label_ids=labels
                )
            )
            try:
                await asyncio.wait_for(entered.wait(), 2)
                labels.append("UNREAD")
            finally:
                release.set()
                await task
    assert server.joined


class HeldClose(httpx.AsyncByteStream):
    def __init__(self):
        self.entered = asyncio.Event()
        self.closing = asyncio.Event()
        self.release = asyncio.Event()
        self.finished = False

    async def __aiter__(self):
        self.entered.set()
        await asyncio.Event().wait()
        yield b""  # pragma: no cover - cancellation supplies the only exit.

    async def aclose(self):
        self.closing.set()
        await self.release.wait()
        self.finished = True


@pytest.mark.asyncio
async def test_repeated_cancellation_keeps_response_close_owned():
    stream = HeldClose()
    async with prepare_httpx_client(httpx.AsyncClient(
        base_url="https://fixture.invalid/",
        trust_env=False,
        transport=httpx.MockTransport(
            lambda r: httpx.Response(
                200, headers={"Content-Type": "application/json"}, stream=stream
            )
        ),
    )) as transport:
        task = asyncio.create_task(GmailOperations(transport, user_id="me").send_raw(MIME))
        try:
            await asyncio.wait_for(stream.entered.wait(), 2)
            task.cancel("first")
            await asyncio.wait_for(stream.closing.wait(), 2)
            task.cancel("second")
            done, _ = await asyncio.wait({task}, timeout=0.02)
            assert not done, "entered response cleanup must stay owned after repeated cancellation"
        finally:
            stream.release.set()
            with pytest.raises(SendCancelled) as caught:
                await task
        assert caught.value.write_outcome is WriteOutcome.UNKNOWN
        assert stream.finished


@pytest.mark.asyncio
async def test_nested_anyio_cancellation_joins_response_close():
    stream = HeldClose()

    async def release():
        await stream.closing.wait()
        await asyncio.sleep(0.02)
        stream.release.set()

    releaser = asyncio.create_task(release())
    async with prepare_httpx_client(httpx.AsyncClient(
        base_url="https://fixture.invalid/",
        trust_env=False,
        transport=httpx.MockTransport(
            lambda r: httpx.Response(
                200, headers={"Content-Type": "application/json"}, stream=stream
            )
        ),
    )) as transport:
        try:
            with anyio.move_on_after(0.02) as scope:
                with pytest.raises(SendCancelled) as caught:
                    await GmailOperations(transport, user_id="me").send_raw(MIME)
                assert caught.value.write_outcome is WriteOutcome.UNKNOWN
            assert scope.cancel_called
            assert stream.finished
        finally:
            stream.release.set()
            await releaser
