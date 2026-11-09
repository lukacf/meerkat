"""Complete provider receipts must match the declared HTTP byte length.

Uses injected HTTPX streams to exercise this public borrowed-client contract,
including clients that have already consumed a response. No network or account.
"""

import json

import httpx
import pytest

from meerkat_toolkit.connectors.gmail import GmailError, GmailOperations
from meerkat_toolkit.connectors.google_calendar import CalendarError, GoogleCalendarOperations
from meerkat_toolkit.connectors.slack import SlackError, SlackOperations
from meerkat_toolkit.httpx_client import prepare_httpx_client


class Stream(httpx.AsyncByteStream):
    def __init__(self, body):
        self.body = body
        self.closed = 0

    async def __aiter__(self):
        # Split UTF-8 at a byte boundary, so character count cannot pass as size.
        for offset in range(0, len(self.body), 3):
            yield self.body[offset:offset + 3]

    async def aclose(self):
        self.closed += 1


@pytest.mark.asyncio
@pytest.mark.parametrize('preconsumed', [False, True])
@pytest.mark.parametrize('length', ['absent', 'exact', 'short', 'long'])
@pytest.mark.parametrize('operation', ['gmail-read', 'gmail-send', 'calendar-read', 'calendar-insert', 'slack-read'])
async def test_complete_json_receipt_length(operation, length, preconsumed):
    if operation == 'gmail-read':
        payload = {'messages': [{'id': 'one', 'threadId': 'thread'}], 'future': 'h\u00e4r'}
    elif operation == 'gmail-send':
        payload = {'id': 'sent', 'threadId': 'thread', 'future': 'h\u00e4r'}
    elif operation == 'calendar-read':
        payload = {'kind': 'calendar#events', 'items': [], 'summary': 'h\u00e4r'}
    elif operation == 'calendar-insert':
        payload = {'id': 'created', 'etag': '"revision"', 'updated': '2026-10-02T01:00:00Z', 'summary': 'h\u00e4r'}
    else:
        payload = {'ok': True, 'messages': [], 'future': 'h\u00e4r'}
    body = json.dumps(payload, ensure_ascii=False).encode('utf-8')
    headers = {'Content-Type': 'application/json'}
    if length != 'absent':
        headers['Content-Length'] = str(len(body) + {'exact': 0, 'short': -1, 'long': 1}[length])
    stream = Stream(body)
    calls = []

    async def send(request):
        calls.append(request)
        response = httpx.Response(200, headers=headers, stream=stream)
        if preconsumed:
            await response.aread()
        return response

    async with prepare_httpx_client(httpx.AsyncClient(
        base_url='https://synthetic.invalid/', transport=httpx.MockTransport(send), trust_env=False,
    )) as client:
        if operation.startswith('gmail'):
            operations = GmailOperations(client, user_id='me')
            call = operations.list_messages_page if operation.endswith('read') else lambda: operations.send_raw(b'To: fixture@example.test\r\n\r\nText')
            error = GmailError
        elif operation.startswith('calendar'):
            operations = GoogleCalendarOperations(client, calendar_id='primary')
            call = operations.list_events_page if operation.endswith('read') else lambda: operations.insert_event({'summary': 'fixture'})
            error = CalendarError
        else:
            operations = SlackOperations(client)
            call = lambda: operations.history_page('C1')
            error = SlackError
        if length in {'short', 'long'}:
            with pytest.raises(error) as caught:
                await call()
            assert caught.value.kind.value == 'invalid_response'
            if operation in {'gmail-send', 'calendar-insert'}:
                assert caught.value.write_outcome.value == 'unknown'
        else:
            assert await call() == payload
        assert not client.is_closed
    assert len(calls) == 1
    assert stream.closed == 1
