"""Generated Telegram bridge is exercised entirely through a fake transport."""
import asyncio
import importlib.util
from pathlib import Path
from types import SimpleNamespace

import pytest

TEMPLATE = Path(__file__).parents[1] / "launchkit/templates/telegram_bridge.py"
module_spec = importlib.util.spec_from_file_location("generated_telegram", TEMPLATE)
telegram = importlib.util.module_from_spec(module_spec)
module_spec.loader.exec_module(telegram)


class FakeTransport:
    def __init__(self):
        self.calls = []
        self.webhook = ""
        self.updates = []
        self.error = None

    async def call(self, method, payload):
        self.calls.append((method, payload))
        if self.error:
            raise self.error
        if method == "getMe":
            return {"id": 1, "is_bot": True}
        if method == "getWebhookInfo":
            return {"url": self.webhook}
        if method == "getUpdates":
            return self.updates
        return {"message_id": 3}


class FakeAgent:
    def __init__(self):
        self.messages = []
        self.answer = "A useful response."
        self.error = None

    async def send(self, text):
        self.messages.append(text)
        if self.error:
            raise self.error
        return SimpleNamespace(completion_baseline="baseline")

    async def wait_for_completion(self, baseline, **kwargs):
        assert baseline == "baseline"
        return self.answer


def bridge(tmp_path, allowed="123,-456"):
    transport = FakeTransport()
    agent = FakeAgent()
    instance = telegram.TelegramBridge(agent, tmp_path / "telegram.json", "fake-token", allowed, transport=transport, clock=lambda: 100)
    return instance, agent, transport


def message(update_id=1, chat=123, text="Hello", date=101, bot=False):
    return {"update_id": update_id, "message": {"chat": {"id": chat}, "text": text, "date": date, "from": {"is_bot": bot}}}


@pytest.mark.parametrize("value", ["", "*", "123,*", "chat", "123,", "0", "123;456"])
def test_allowlist_rejects_invalid_values(value):
    with pytest.raises(ValueError, match="numeric"):
        telegram.chat_allowlist(value)


def test_allowlist_accepts_private_and_group_ids():
    assert telegram.chat_allowlist("123, -456") == {123, -456}


@pytest.mark.asyncio
async def test_only_allowlisted_human_text_reaches_agent(tmp_path):
    instance, agent, transport = bridge(tmp_path)
    inputs = [
        message(1, chat=999), message(2, bot=True), message(3, date=99),
        {"update_id": 4, "edited_message": message()["message"]},
        {"update_id": 5, "message": {"chat": {"id": 123}, "from": {"is_bot": False}, "date": 101, "photo": []}},
        message(6),
    ]
    for update in inputs:
        await instance.handle_update(update)
    assert agent.messages == ["Hello"]
    assert transport.calls == [("sendMessage", {"chat_id": 123, "text": "A useful response."})]
    assert instance.next_offset == 7
    assert instance.state_path.stat().st_mode & 0o777 == 0o600


@pytest.mark.asyncio
async def test_claimed_update_is_not_redelivered_after_restart(tmp_path):
    instance, agent, transport = bridge(tmp_path)
    await instance.handle_update(message(12))
    restored = telegram.TelegramBridge(agent, instance.state_path, "fake", "123", transport=transport, clock=lambda: 500)
    assert restored.first_started_at == 100
    await restored.handle_update(message(12))
    assert agent.messages == ["Hello"]
    await restored.handle_update(message(13, date=400))
    assert agent.messages == ["Hello", "Hello"]


@pytest.mark.asyncio
async def test_provider_failure_returns_sanitized_same_chat_reply(tmp_path):
    instance, agent, transport = bridge(tmp_path)
    agent.error = RuntimeError("SECRET PROVIDER REQUEST CONTEXT")
    await instance.handle_update(message(chat=-456))
    assert transport.calls[0][1]["chat_id"] == -456
    assert "SECRET" not in transport.calls[0][1]["text"]


@pytest.mark.asyncio
async def test_existing_webhook_is_refused_without_deleting_it(tmp_path):
    instance, _, transport = bridge(tmp_path)
    transport.webhook = "https://existing.example/webhook"
    with pytest.raises(telegram.TelegramError, match="existing webhook"):
        await instance.check()
    assert [method for method, _ in transport.calls] == ["getMe", "getWebhookInfo"]


def test_reply_splitting_respects_utf16_limit():
    text = "\U0001f331" * 5000
    chunks = telegram.split_reply(text)
    assert "".join(chunks) == text
    assert all(len(chunk.encode("utf-16-le")) // 2 <= 3500 for chunk in chunks)


@pytest.mark.asyncio
async def test_reply_chunks_stay_in_admitted_chat(tmp_path):
    instance, agent, transport = bridge(tmp_path)
    agent.answer = "x" * 9000
    await instance.handle_update(message())
    assert len(transport.calls) == 3
    assert all(payload["chat_id"] == 123 for _, payload in transport.calls)
    assert "".join(payload["text"] for _, payload in transport.calls) == agent.answer


@pytest.mark.asyncio
async def test_stop_interrupts_retry_backoff(tmp_path):
    instance, _, transport = bridge(tmp_path)
    calls = 0
    async def failing(method, payload):
        nonlocal calls
        if method == "getMe": return {}
        if method == "getWebhookInfo": return {"url": ""}
        calls += 1
        raise telegram.TelegramError("temporary", retryable=True)
    transport.call = failing
    stop = asyncio.Event()
    task = asyncio.create_task(instance.run(stop))
    await asyncio.sleep(0.01)
    stop.set()
    await asyncio.wait_for(task, 0.5)
    assert calls == 1


@pytest.mark.asyncio
async def test_unretryable_error_ends_bridge(tmp_path):
    instance, _, transport = bridge(tmp_path)
    transport.error = telegram.TelegramError("credential refused")
    with pytest.raises(telegram.TelegramError, match="credential refused"):
        await instance.run(asyncio.Event())
