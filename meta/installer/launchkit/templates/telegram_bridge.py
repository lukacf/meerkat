"""Optional Telegram bridge with explicit chat admission and durable receive cursors."""
from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
import re
import time
import urllib.error
import urllib.request


class TelegramError(RuntimeError):
    def __init__(self, message, *, retryable=False):
        super().__init__(message)
        self.retryable = retryable


def chat_allowlist(value):
    pieces = [piece.strip() for piece in value.split(",")]
    if not pieces or any(not re.fullmatch(r"-?[1-9][0-9]{0,19}", piece) for piece in pieces):
        raise ValueError("TELEGRAM_ALLOWED_CHAT_IDS requires comma-separated numeric chat IDs; wildcards are not supported")
    return frozenset(int(piece) for piece in pieces)


def split_reply(text, limit=3500):
    """Stay below Telegram's 4096 UTF-16-unit limit, including non-BMP text."""
    chunks = []
    current = []
    units = 0
    for character in str(text):
        width = 2 if ord(character) > 0xFFFF else 1
        if units + width > limit:
            chunks.append("".join(current))
            current, units = [], 0
        current.append(character)
        units += width
    if current:
        chunks.append("".join(current))
    return chunks or ["The coordinator finished without a text reply."]


class TelegramTransport:
    def __init__(self, token):
        if not re.fullmatch(r"[0-9]+:[A-Za-z0-9_-]+", token):
            raise ValueError("TELEGRAM_BOT_TOKEN has an invalid format")
        self._base = "https://api.telegram.org/bot" + token + "/"

    def _request(self, method, payload):
        request = urllib.request.Request(self._base + method, data=json.dumps(payload).encode(),
            headers={"Content-Type": "application/json", "User-Agent": "Meta-Forge/1"})
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                raw = response.read(1_000_001)
            if len(raw) > 1_000_000:
                raise TelegramError("Telegram response exceeded its size limit")
            data = json.loads(raw)
        except urllib.error.HTTPError as error:
            code = error.code
            if code == 409:
                raise TelegramError("Telegram has an existing webhook or another bot poller. Use a dedicated bot and remove the conflict yourself.") from None
            if code in {401, 403}:
                raise TelegramError("Telegram authentication or chat access was refused. Check the bot token and allowed chat.") from None
            raise TelegramError(f"Telegram returned HTTP {code}", retryable=code == 429 or code >= 500) from None
        except (urllib.error.URLError, TimeoutError, OSError):
            raise TelegramError("Telegram could not be reached", retryable=True) from None
        except (ValueError, TypeError):
            raise TelegramError("Telegram returned an invalid response") from None
        if not data.get("ok"):
            code = data.get("error_code")
            if code == 409:
                raise TelegramError("Telegram has an existing webhook or another bot poller. Use a dedicated bot.")
            raise TelegramError("Telegram refused the request", retryable=code == 429 or isinstance(code, int) and code >= 500)
        return data.get("result")

    async def call(self, method, payload):
        return await asyncio.to_thread(self._request, method, payload)


class TelegramBridge:
    def __init__(self, agent, state_path, token, allowed_chats, *, transport=None, clock=time.time):
        self.agent = agent
        self.state_path = Path(state_path)
        self.allowed_chats = chat_allowlist(allowed_chats)
        self.transport = transport or TelegramTransport(token)
        self.clock = clock
        self.next_offset = None
        self.first_started_at = int(clock())
        if self.state_path.exists():
            state = json.loads(self.state_path.read_text())
            offset = state.get("next_offset")
            started = state.get("first_started_at")
            if offset is not None and (type(offset) is not int or offset < 0):
                raise ValueError("Telegram receive state is invalid")
            if type(started) is not int or started < 0:
                raise ValueError("Telegram receive state is invalid")
            self.next_offset, self.first_started_at = offset, started
        else:
            self._save_cursor()

    def _save_cursor(self):
        self.state_path.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
        temporary = self.state_path.with_suffix(".pending")
        temporary.write_text(json.dumps({"next_offset": self.next_offset, "first_started_at": self.first_started_at}) + "\n")
        temporary.chmod(0o600)
        temporary.replace(self.state_path)

    async def check(self):
        await self.transport.call("getMe", {})
        webhook = await self.transport.call("getWebhookInfo", {})
        if webhook.get("url"):
            raise TelegramError("This bot has an existing webhook. Use a dedicated bot or remove that webhook yourself before starting.")

    async def answer(self, text):
        from meerkat_mobkit.errors import RpcError
        admission = await self.agent.send(text)
        baseline = admission.completion_baseline
        if baseline is None:
            raise RuntimeError("The coordinator did not return a completion baseline")
        deadline = asyncio.get_running_loop().time() + 120
        while True:
            remaining = deadline - asyncio.get_running_loop().time()
            if remaining <= 0:
                raise TimeoutError("The coordinator did not finish in time")
            try:
                return await self.agent.wait_for_completion(baseline, timeout=remaining)
            except RpcError as error:
                if "observation_lane_saturated" not in str(error):
                    raise
                await asyncio.sleep(0.5)

    async def handle_update(self, update):
        update_id = update.get("update_id")
        if type(update_id) is not int or update_id < 0:
            raise TelegramError("Telegram returned an invalid update ID")
        if self.next_offset is not None and update_id < self.next_offset:
            return
        # Claim before delivery, so an interrupted process never redelivers a
        # user request after restart. A crash can lose a reply; it cannot replay work.
        self.next_offset = update_id + 1
        self._save_cursor()
        message = update.get("message")
        if not isinstance(message, dict):
            return
        chat_id = message.get("chat", {}).get("id")
        sender = message.get("from", {})
        text = message.get("text")
        if (type(chat_id) is not int or chat_id not in self.allowed_chats
                or sender.get("is_bot") is not False or not isinstance(text, str)
                or not text.strip() or message.get("date", 0) < self.first_started_at):
            return
        if len(text) > 8000:
            reply = "That message is too long. Please send fewer than 8000 characters."
        else:
            try:
                reply = await self.answer(text)
            except Exception:
                # SDK/provider errors can carry private context; never forward it.
                reply = "Your coordinator could not finish this request. Check the app's local status and try again."
        for chunk in split_reply(reply or "The coordinator finished without a text reply."):
            await self.transport.call("sendMessage", {"chat_id": chat_id, "text": chunk})

    async def run(self, stop):
        await self.check()
        print("Telegram bridge ready for explicitly allowed chats. Older messages from before the first startup are skipped.", flush=True)
        failures = 0
        while not stop.is_set():
            try:
                payload = {"timeout": 20, "limit": 30, "allowed_updates": ["message"]}
                if self.next_offset is not None:
                    payload["offset"] = self.next_offset
                updates = await self.transport.call("getUpdates", payload)
                for update in updates:
                    if stop.is_set():
                        return
                    await self.handle_update(update)
                failures = 0
            except TelegramError as error:
                failures += 1
                if not error.retryable or failures >= 5:
                    raise
                print("Telegram connection interrupted; retrying.", flush=True)
                try:
                    await asyncio.wait_for(stop.wait(), timeout=min(2 ** failures, 30))
                except asyncio.TimeoutError:
                    pass
