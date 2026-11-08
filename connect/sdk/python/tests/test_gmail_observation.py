"""Actual Gmail HTTPX operation plus stateless observation controls.

Mock provider and account admission only. No live account, native runnable,
source advance, delivery receipt or consumer completion is claimed here.
"""
import asyncio
from contextlib import asynccontextmanager
from dataclasses import asdict
import json
import unittest

import httpx

from meerkat_toolkit.connector_auth import VerifiedConnectorAccount
from meerkat_toolkit.connectors.gmail import FailureKind, GmailError, GmailOperations
from meerkat_toolkit.httpx_client import prepare_httpx_client
from meerkat_toolkit.observations.gmail import (
    GmailInboxBinding, GmailInboxObservation, GmailObservationError, GmailRebaselineRequired,
    GmailObservationGap,
)

ACCOUNT = VerifiedConnectorAccount("https://accounts.google.com", "google-subject", "opaque-subject")


def addition(index, labels=("INBOX",)):
    value = {"id": f"m{index:04}", "threadId": f"t{index:04}"}
    if labels is not None:
        value["labelIds"] = list(labels)
    return {"message": value}


@asynccontextmanager
async def fixture(handler, *, check=None):
    requests = []

    async def receive(request):
        requests.append(request)
        result = handler(request)
        if isinstance(result, httpx.Response):
            return result
        return httpx.Response(200, json=result)

    async def current():
        return ACCOUNT

    async with httpx.AsyncClient(base_url="https://gmail.googleapis.com/", follow_redirects=False,
                                 trust_env=False, transport=httpx.MockTransport(receive)) as client:
        prepare_httpx_client(client)
        operations = GmailOperations(client, user_id="me")
        binding = GmailInboxBinding(ACCOUNT, ("mail.inbox",), "gmail-inbox")
        adapter = GmailInboxObservation(operations, binding, check_current=check or current)
        yield adapter, operations, binding, requests, check or current


class GmailObservationTests(unittest.IsolatedAsyncioTestCase):
    async def test_baseline_is_profile_checkpoint_without_historical_replay(self):
        async with fixture(lambda _: {"emailAddress": "not-the-subject@example.invalid", "historyId": "10"}) as (adapter, _, binding, requests, _):
            page = await adapter.collect(None)
            self.assertEqual(page.items, ())
            self.assertTrue(page.final)
            self.assertEqual(page.coverage, "complete")
            self.assertIsNone(page.observed, "profile baseline is not a delta request")
            self.assertEqual(json.loads(page.cursor)["history_id"], "10")
            self.assertEqual([request.url.path for request in requests], ["/gmail/v1/users/me/profile"])
            self.assertEqual(binding.principal()["subject"], "opaque-subject")
            self.assertNotIn("example.invalid", page.cursor)

    async def test_fixed_projection_ignores_label_only_changes_sent_and_drafts(self):
        def provider(request):
            if request.url.path.endswith("profile"):
                return {"emailAddress": "selected@example.invalid", "historyId": "10"}
            if "/messages/" in request.url.path:
                self.assertEqual(request.url.params["format"], "minimal")
                return {"id": "m0000", "threadId": "t0000", "labelIds": ["INBOX", "UNREAD"]}
            self.assertEqual(dict(request.url.params), {"startHistoryId": "10", "maxResults": "1", "historyTypes": "messageAdded"})
            return {"historyId": "30", "history": [{"id": "20", "messagesAdded": [
                addition(0, None), addition(1), addition(2, ("INBOX", "SENT")), addition(3, ("INBOX", "DRAFT")),
                addition(4, ("UNREAD",)),
            ], "labelsAdded": [{"message": {"id": "old", "threadId": "old"}, "labelIds": ["INBOX"]}]}]}
        async with fixture(provider) as (adapter, _, _, requests, _):
            baseline = await adapter.collect(None)
            page = await adapter.collect(baseline.cursor)
            self.assertEqual([item["item_id"] for item in page.items], ["m0000", "m0001"])
            self.assertTrue(page.final)
            self.assertEqual(page.messages_examined, 5)
            self.assertEqual(len(requests), 3)
            for item in page.items:
                self.assertNotIn("UNREAD", json.dumps(item))
                self.assertNotIn("label", item["body"])
                self.assertEqual(item["change"], "upsert")
            # Re-read before native advance is byte-stable, not a seen-set skip.
            again = await adapter.collect(baseline.cursor)
            self.assertEqual(page, again)

    async def test_large_single_history_record_is_bounded_across_reconstruction(self):
        def provider(request):
            if request.url.path.endswith("profile"):
                return {"emailAddress": "selected@example.invalid", "historyId": "10"}
            return {"historyId": "40", "history": [{"id": "20", "messagesAdded": [addition(i) for i in range(205)]}]}
        async with fixture(provider) as (adapter, operations, binding, requests, check):
            token = (await adapter.collect(None)).cursor
            pages = []
            for _ in range(3):
                adapter = GmailInboxObservation(operations, binding, check_current=check)
                page = await adapter.collect(token)
                pages.append(page)
                token = page.cursor
                self.assertLessEqual(len(token.encode()), 8192)
                self.assertLessEqual(len(json.dumps(asdict(page), ensure_ascii=True).encode()), 1048576)
                self.assertLessEqual(sum(len(item["body"].encode()) for item in page.items), 1048576)
                for item in page.items:
                    self.assertLessEqual(len(item["body"].encode()), 16384)
                    self.assertLessEqual(len(item["item_id"].encode()), 256)
                    self.assertLessEqual(len(item["version"].encode()), 256)
            self.assertEqual([len(page.items) for page in pages], [100, 100, 5])
            self.assertEqual([page.final for page in pages], [False, False, True])
            self.assertEqual([page.observed for page in pages], [None, None, {"kind": "delta"}])
            self.assertEqual([page.page_index for page in pages], [0, 1, 2])
            self.assertEqual(json.loads(token)["history_id"], "40")
            self.assertEqual(len({item["item_id"] for page in pages for item in page.items}), 205)
            self.assertEqual(len(requests), 4)
            self.assertTrue(all(request.url.params.get("maxResults") == "1" for request in requests[1:]))

    async def test_provider_page_is_not_a_checkpoint_and_no_change_is_healthy(self):
        def provider(request):
            if request.url.path.endswith("profile"):
                return {"emailAddress": "selected@example.invalid", "historyId": "10"}
            if request.url.params["startHistoryId"] == "50":
                return {"historyId": "50"}
            if "pageToken" not in request.url.params:
                return {"historyId": "50", "nextPageToken": "opaque+/= next", "history": [{"id": "20", "messagesAdded": [addition(1)]}]}
            self.assertEqual(request.url.params["pageToken"], "opaque+/= next")
            return {"historyId": "50", "history": [{"id": "30", "messagesAdded": [addition(2)]}]}
        async with fixture(provider) as (adapter, _, _, _, _):
            token = (await adapter.collect(None)).cursor
            first = await adapter.collect(token)
            self.assertFalse(first.final)
            self.assertEqual(json.loads(first.cursor)["history_id"], "10")
            last = await adapter.collect(first.cursor)
            self.assertTrue(last.final)
            self.assertEqual(json.loads(last.cursor)["history_id"], "50")
            idle = await adapter.collect(last.cursor)
            self.assertTrue(idle.final)
            self.assertEqual(idle.items, ())
            self.assertIsNone(idle.coverage)
            self.assertEqual(idle.observed, {"kind": "delta"}, "confirmed empty delta stays distinct from empty snapshot")

    async def test_changed_partial_record_refuses_without_skipping_remainder(self):
        changed = False
        def provider(request):
            if request.url.path.endswith("profile"):
                return {"emailAddress": "selected@example.invalid", "historyId": "10"}
            return {"historyId": "40", "history": [{"id": "20", "messagesAdded": [addition(i) for i in range(102 if changed else 101)]}]}
        async with fixture(provider) as (adapter, _, _, _, _):
            first = await adapter.collect((await adapter.collect(None)).cursor)
            changed = True
            with self.assertRaisesRegex(GmailObservationError, "history_page_changed"):
                await adapter.collect(first.cursor)

    async def test_expired_history_requires_explicit_native_rebaseline_and_gap(self):
        def provider(request):
            if request.url.path.endswith("profile"):
                return {"emailAddress": "selected@example.invalid", "historyId": "10"}
            return httpx.Response(404, json={"error": "PRIVATE PROVIDER BODY"})
        async with fixture(provider) as (adapter, _, _, requests, _):
            token = (await adapter.collect(None)).cursor
            with self.assertRaises(GmailRebaselineRequired) as caught:
                await adapter.collect(token)
            self.assertEqual(str(caught.exception), "cursor_expired")
            self.assertEqual(len(requests), 2, "no automatic full sync or replay")

    async def test_missing_labels_are_never_treated_as_an_empty_mailbox(self):
        def provider(request):
            if request.url.path.endswith("profile"):
                return {"emailAddress": "selected@example.invalid", "historyId": "10"}
            if "/messages/" in request.url.path:
                return {"id": "m0000", "threadId": "t0000"}
            return {"historyId": "40", "history": [{"id": "20", "messagesAdded": [addition(0, None)]}]}
        async with fixture(provider) as (adapter, _, _, _, _):
            token = (await adapter.collect(None)).cursor
            with self.assertRaisesRegex(GmailObservationError, "message_labels_unavailable"):
                await adapter.collect(token)

    async def test_account_switch_refuses_result_and_cursor_reuse(self):
        account = ACCOUNT
        async def current():
            return account
        def provider(request):
            nonlocal account
            account = VerifiedConnectorAccount(ACCOUNT.issuer, ACCOUNT.strategy_id, "other-subject")
            return {"emailAddress": "selected@example.invalid", "historyId": "10"}
        async with fixture(provider, check=current) as (adapter, _, _, requests, _):
            with self.assertRaisesRegex(GmailObservationError, "principal_changed"):
                await adapter.collect(None)
            self.assertEqual(len(requests), 1)

    async def test_cursor_bound_and_selection_change_refuse_before_provider_read(self):
        async with fixture(lambda _: {"emailAddress": "selected@example.invalid", "historyId": "10"}) as (adapter, ops, binding, requests, check):
            token = (await adapter.collect(None)).cursor
            other = GmailInboxObservation(ops, GmailInboxBinding(ACCOUNT, ("different-topic",), binding.source_id), check_current=check)
            with self.assertRaisesRegex(GmailObservationError, "invalid_cursor"):
                await other.collect(token)
            with self.assertRaisesRegex(GmailObservationError, "invalid_cursor"):
                await adapter.collect("x" * 8193)
            self.assertEqual(len(requests), 1)

    async def test_oversized_provider_token_is_refused_not_truncated(self):
        def provider(request):
            if request.url.path.endswith("profile"):
                return {"emailAddress": "selected@example.invalid", "historyId": "10"}
            return {"historyId": "40", "nextPageToken": "x" * 8100}
        async with fixture(provider) as (adapter, _, _, _, _):
            token = (await adapter.collect(None)).cursor
            with self.assertRaisesRegex(GmailObservationError, "cursor_too_large"):
                await adapter.collect(token)

    async def test_history_operation_rejects_record_count_and_does_not_follow(self):
        async with fixture(lambda _: {"historyId": "40", "history": [{"id": "20"}, {"id": "30"}]}) as (_, operations, _, requests, _):
            with self.assertRaises(GmailError) as caught:
                await operations.history_page(start_history_id="10", max_results=1)
            self.assertEqual(caught.exception.kind, FailureKind.INVALID_RESPONSE)
            self.assertEqual(len(requests), 1)

    def test_existing_service_profile_exposes_read_only_history_operation(self):
        from meerkat_toolkit.adapters.services_config import validate_configuration, tool_descriptions
        configuration = validate_configuration({"profile": "gmail", "operations": ["history_page"],
            "config": {"token_env": "SELECTED_TOKEN", "user_id": "me", "timeout_seconds": 20,
                       "max_response_bytes": 2097152, "max_message_bytes": 4194304}})
        tool, = tool_descriptions(configuration)
        self.assertEqual(tool["name"], "gmail_history_page")
        self.assertEqual(tool["effect"], "read")
        self.assertEqual(tool["inputSchema"]["required"], ["start_history_id"])
        self.assertIn("history records", tool["description"])


    async def test_deleted_during_label_read_reports_gap_without_cursor_or_tombstone(self):
        def provider(request):
            if request.url.path.endswith("profile"):
                return {"emailAddress": "selected@example.invalid", "historyId": "10"}
            if "/messages/" in request.url.path:
                return httpx.Response(404, json={"error": "PRIVATE deletion diagnostic"})
            return {"historyId": "40", "history": [{"id": "20", "messagesAdded": [addition(0, None)]}]}
        async with fixture(provider) as (adapter, _, _, requests, _):
            token = (await adapter.collect(None)).cursor
            with self.assertRaises(GmailObservationGap) as caught:
                await adapter.collect(token)
            self.assertEqual(str(caught.exception), "message_unavailable")
            self.assertIsNone(caught.exception.__context__)
            self.assertFalse(hasattr(caught.exception, "cursor"))
            self.assertFalse(isinstance(caught.exception, GmailRebaselineRequired))
            self.assertEqual(len(requests), 3, "no retry, cursor advance or implicit rebaseline")

    async def test_history_transport_bounds_raw_record_before_json_and_closes_stream(self):
        class OversizeRecord(httpx.AsyncByteStream):
            read_chunks = 0
            closed = False
            async def __aiter__(self):
                # A single history record can exceed maxResults' intended bound.
                yield b'{"historyId":"40","history":[{"id":"20","messagesAdded":['
                for _ in range(100):
                    self.read_chunks += 1
                    yield b" " * 65536
            async def aclose(self):
                self.closed = True
        stream = OversizeRecord()
        def provider(request):
            if request.url.path.endswith("profile"):
                return {"emailAddress": "selected@example.invalid", "historyId": "10"}
            return httpx.Response(200, headers={"content-type": "application/json"}, stream=stream)
        async with fixture(provider) as (adapter, _, _, _, _):
            token = (await adapter.collect(None)).cursor
            with self.assertRaises(GmailError) as caught:
                await adapter.collect(token)
            self.assertEqual(caught.exception.kind, FailureKind.RESPONSE_TOO_LARGE)
            self.assertTrue(stream.closed)
            self.assertLessEqual(stream.read_chunks, 33)

    async def test_final_serialization_bounds_and_invalid_unicode_refuse(self):
        def provider(request):
            if request.url.path.endswith("profile"):
                return {"emailAddress": "selected@example.invalid", "historyId": "10"}
            return {"historyId": "40", "history": [{"id": "20", "messagesAdded": [addition(i) for i in range(100)]}]}
        async with fixture(provider) as (_, ops, _, _, check):
            topics = tuple("\u00ff" * 126 + f"{i:02}" for i in range(16))
            adapter = GmailInboxObservation(ops, GmailInboxBinding(ACCOUNT, topics, "gmail-inbox"), check_current=check)
            token = (await adapter.collect(None)).cursor
            with self.assertRaisesRegex(GmailObservationError, "observation_page_too_large"):
                await adapter.collect(token)
        with self.assertRaisesRegex(GmailObservationError, "invalid_observation_data") as caught:
            GmailInboxBinding(ACCOUNT, ("\ud800",), "gmail-inbox")
        self.assertIsNone(caught.exception.__context__)

    async def test_account_switch_after_history_refuses_items_and_next_cursor(self):
        account = ACCOUNT
        async def check():
            return account
        def provider(request):
            nonlocal account
            if request.url.path.endswith("profile"):
                return {"emailAddress": "selected@example.invalid", "historyId": "10"}
            account = VerifiedConnectorAccount(ACCOUNT.issuer, ACCOUNT.strategy_id, "replacement")
            return {"historyId": "40", "history": [{"id": "20", "messagesAdded": [addition(0)]}]}
        async with fixture(provider, check=check) as (adapter, _, _, _, _):
            token = (await adapter.collect(None)).cursor
            with self.assertRaisesRegex(GmailObservationError, "principal_changed"):
                await adapter.collect(token)

    async def test_cancelled_connection_check_has_no_private_diagnostic_or_result(self):
        entered = asyncio.Event()
        async def check():
            entered.set()
            await asyncio.Event().wait()
        async with fixture(lambda _: self.fail("no provider call"), check=check) as (adapter, _, _, _, _):
            task = asyncio.create_task(adapter.collect(None))
            await entered.wait()
            task.cancel("PRIVATE cancellation text")
            with self.assertRaises(asyncio.CancelledError) as caught:
                await task
            self.assertNotIn("PRIVATE", str(caught.exception))
            self.assertIsNone(caught.exception.__context__)


    async def test_account_switch_during_provider_refusal_is_not_cursor_expiry(self):
        account = ACCOUNT
        async def check():
            return account
        def provider(request):
            nonlocal account
            if request.url.path.endswith("profile"):
                return {"emailAddress": "selected@example.invalid", "historyId": "10"}
            account = VerifiedConnectorAccount(ACCOUNT.issuer, ACCOUNT.strategy_id, "replacement")
            return httpx.Response(404, json={"error": "PRIVATE body"})
        async with fixture(provider, check=check) as (adapter, _, _, _, _):
            token = (await adapter.collect(None)).cursor
            with self.assertRaisesRegex(GmailObservationError, "principal_changed") as caught:
                await adapter.collect(token)
            self.assertIsNone(caught.exception.__context__)
