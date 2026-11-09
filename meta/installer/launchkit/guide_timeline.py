"""Project setup-scoped native console events into the embedded conversation."""
from __future__ import annotations

import asyncio
import copy
import json
import re

from .architect import _native_request


def cursor_number(value):
    if isinstance(value, int) and not isinstance(value, bool):
        return value
    if isinstance(value, str) and value.startswith("console:"):
        try:
            return int(value.removeprefix("console:"))
        except ValueError:
            pass
    raise ValueError("Invalid native conversation cursor.")


def visible_content(binding):
    refs = binding.get("attachments", [])
    if not refs:
        return binding["user_text"]
    links = []
    for ref in refs:
        label = re.sub(r"([\\\[\]()*_`])", r"\\\1", ref["filename"])
        links.append(f"[{label}]({ref['url']})")
    content = [{"type": "text", "text": binding["user_text"] + "\n\n" + "\n\n".join(links)}]
    content.extend({"type": "image", "media_type": ref["media_type"], "url": ref["url"], "alt": ref["filename"]}
                   for ref in refs if ref["media_type"].startswith("image/"))
    return content


def project_frame(frame, binding):
    """Retain native tool/reasoning events; replace host-only request scaffolding."""
    result = copy.deepcopy(frame)
    payload = result.get("payload")
    if not isinstance(payload, dict):
        return result
    if result.get("kind") == "user_input":
        payload["content"] = visible_content(binding)
        payload["guide_message_id"] = binding["user_message_id"]
    elif result.get("kind") in {"run_started", "interaction_started"}:
        if "input" in payload:
            payload["input"] = visible_content(binding)
        if "content" in payload:
            payload["content"] = visible_content(binding)
    elif result.get("kind") == "frame_updated" and isinstance(payload.get("frame"), dict):
        payload["frame"] = project_frame(payload["frame"], binding)
    return result


def legacy_binding(frame, state):
    """Recover the setup IDs already sent as explicit native request context."""
    if frame.get("kind") != "user_input":
        return None
    text = frame.get("payload", {}).get("content")
    if isinstance(text, list):
        text = next((part.get("text") for part in text if isinstance(part, dict) and part.get("type") == "text"), None)
    marker = "Current setup turn:\n"
    if not isinstance(text, str) or marker not in text:
        return None
    try:
        context, _ = json.JSONDecoder().raw_decode(text.split(marker, 1)[1])
    except (ValueError, TypeError):
        return None
    if not isinstance(context, dict) or context.get("guide_id") != state["id"]:
        return None
    message = context.get("current_user_message", {})
    saved = next((item for item in state["messages"] if item.get("role") == "user"
                  and isinstance(message, dict) and item.get("id") == message.get("id")), None)
    if saved is None:
        return None
    if not isinstance(message.get("text"), str) or not frame.get("interaction_id"):
        return None
    return {"interaction_id": frame["interaction_id"], "cursor": frame["cursor"],
            "user_message_id": saved["id"], "user_text": saved["text"],
            **({"attachments": copy.deepcopy(saved["attachments"])} if saved.get("attachments") else {}),
            "timestamp_ms": frame.get("timestamp_ms")}


class GuideTimeline:
    def __init__(self, store, architect):
        self.store = store
        self.architect = architect
        self.architect.on_interaction_admitted = self.admitted
        self.backfilled = {}
        self.backfill_lock = asyncio.Lock()

    async def admitted(self, context, accepted):
        message = context["current_user_message"]
        binding = {"interaction_id": accepted["interaction_id"], "cursor": accepted["cursor"],
                   "user_message_id": message["id"], "user_text": message["text"],
                   **({"attachments": copy.deepcopy(message["attachments"])} if message.get("attachments") else {})}
        request_id = context.get("operation_request_id")
        if isinstance(request_id, str) and re.fullmatch(r"[0-9a-f]{64}", request_id):
            binding["operation_request_id"] = request_id
            operation = await asyncio.to_thread(self.store.operation, context["guide_id"])
            # The running guide task owns this admission. An identical retry
            # has the same request hash but a different host-generated start.
            if (operation and operation.get("status") == "running"
                    and operation.get("request_id") == request_id
                    and isinstance(operation.get("started_at"), str) and operation["started_at"]):
                binding["operation_started_at"] = operation["started_at"]
        await asyncio.to_thread(self.store.save_interaction, context["guide_id"], binding)

    async def _page(self, client, after, limit=500):
        base = self.architect.runtime.rust_http_base_url
        response = await _native_request(client, "get", base + "/console/timeline", params={
            "identity": "architect:main", "after": after, "limit": limit})
        response.raise_for_status()
        return response.json()

    async def _backfill(self, client, state):
        identity = state["id"]
        user_ids = {message["id"] for message in state["messages"] if message.get("role") == "user"}
        async with self.backfill_lock:
            if user_ids <= self.backfilled.get(identity, set()):
                return
            after = "console:0"
            while True:
                page = await self._page(client, after)
                frames = page.get("frames", [])
                for frame in frames:
                    binding = legacy_binding(frame, state)
                    if binding:
                        await asyncio.to_thread(self.store.save_interaction, identity, binding)
                cursor = page.get("next_cursor")
                if not frames or not cursor or cursor == after or cursor_number(cursor) >= cursor_number(page.get("latest_cursor", cursor)):
                    break
                after = cursor
            self.backfilled.setdefault(identity, set()).update(user_ids)

    async def read(self, identity, *, after=None, limit=500):
        state = await asyncio.to_thread(self.store.get, identity)
        if state is None:
            raise LookupError("This setup could not be found.")
        if after is not None:
            cursor_number(after)
        bindings = await asyncio.to_thread(self.store.interactions, identity)
        has_history = any(message.get("role") == "user" for message in state["messages"])
        if not bindings and not has_history:
            return {"available": False, "frames": [], "bindings": [], "next_cursor": after, "exhausted": True}
        await self.architect.start()
        import httpx
        async with httpx.AsyncClient(timeout=20, trust_env=False) as client:
            bound_messages = {binding["user_message_id"] for binding in bindings}
            unbound_messages = {message["id"] for message in state["messages"]
                                if message.get("role") == "user" and message.get("id") not in bound_messages}
            if unbound_messages - self.backfilled.get(identity, set()):
                await self._backfill(client, state)
                bindings = await asyncio.to_thread(self.store.interactions, identity)
            if not bindings:
                return {"available": True, "frames": [], "bindings": [], "next_cursor": after, "exhausted": True}
            messages = state["messages"]
            for binding in bindings:
                index = next((i for i, message in enumerate(messages) if message.get("id") == binding["user_message_id"]), None)
                if index is not None and index + 1 < len(messages) and messages[index + 1].get("role") == "assistant":
                    binding["assistant_message_id"] = messages[index + 1].get("id")
            by_interaction = {binding["interaction_id"]: binding for binding in bindings}
            start = after or f"console:{max(0, min(cursor_number(binding['cursor']) for binding in bindings) - 1)}"
            page = await self._page(client, start, limit)
            frames = [project_frame(frame, by_interaction[frame["interaction_id"]])
                      for frame in page.get("frames", []) if frame.get("interaction_id") in by_interaction]
            next_cursor = page.get("next_cursor") or start
            latest = page.get("latest_cursor") or next_cursor
            return {"available": True, "frames": frames, "bindings": bindings,
                    "next_cursor": next_cursor, "latest_cursor": latest,
                    "exhausted": cursor_number(next_cursor) >= cursor_number(latest)}
