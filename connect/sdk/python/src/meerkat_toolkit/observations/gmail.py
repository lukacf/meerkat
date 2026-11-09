"""Bounded Gmail INBOX message-added observations with no progress store.

The trusted host supplies an already authorized operations adapter and a live
connection-owner check. That check must authorize the selected collection and
return Core's verified account on every read. A status observation alone is not
permission. This module never resolves credentials, creates a native runnable,
advances Meta's cursor, or delivers a notification.
"""
from __future__ import annotations

import asyncio
from dataclasses import dataclass, field
import hashlib
import json
from typing import Any, Awaitable, Callable, Protocol

from ..connector_auth import VerifiedConnectorAccount
from ..connectors.gmail import FailureKind, GmailError

PROJECTION = "gmail-inbox-message-added-v1"
MAX_CURSOR_BYTES = 8192
MAX_ITEMS = 100
MAX_BODY_BYTES = 16_384
MAX_PAGE_BYTES = 1_048_576


class GmailObservationError(RuntimeError):
    """Fixed diagnostics with no provider body, account, cursor or URL."""
    def __init__(self, code: str):
        self.code = code
        super().__init__(code)


class GmailRebaselineRequired(GmailObservationError):
    """The host must declare a native source gap and request a new baseline."""
    def __init__(self):
        super().__init__("cursor_expired")


class GmailObservationGap(GmailObservationError):
    """Incomplete evidence, requiring explicit native gap disposition.

    No next cursor or inferred tombstone is returned. In the current native
    contract a host may authorize an operator rebaseline; this is not an expired
    history cursor and must not be mislabeled as one.
    """
    def __init__(self):
        super().__init__("message_unavailable")


class GmailReadOperations(Protocol):
    """The existing Gmail operation shape, called under host-owned authority."""
    async def get_profile(self) -> dict[str, Any]: ...
    async def history_page(self, *, start_history_id: str, page_token: str | None,
                           max_results: int, history_types: tuple[str, ...]) -> dict[str, Any]: ...
    async def get_message(self, message_id: str, *, format: str) -> dict[str, Any]: ...


def _json(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True,
                      allow_nan=False)


def _text(value: Any, bound: int = 256) -> str:
    valid = type(value) is str and bool(value)
    if valid:
        try:
            valid = (len(value.encode("utf-8")) <= bound
                     and not any(ord(c) < 32 or 127 <= ord(c) <= 159 for c in value))
        except UnicodeError:
            valid = False
    if not valid:
        raise GmailObservationError("invalid_observation_data")
    return value


def _history(value: Any) -> str:
    value = _text(value, 256 - len(PROJECTION) - 1)
    if not value.isascii() or not value.isdecimal():
        raise GmailObservationError("invalid_history_id")
    return value


def _message_id(value: Any) -> str:
    value = _text(value)
    if not value.isascii() or any(not (c.isalnum() or c in "_-") for c in value):
        raise GmailObservationError("invalid_message_id")
    return value


def _object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise GmailObservationError("invalid_cursor")
        value[key] = item
    return value


@dataclass(frozen=True, repr=False)
class GmailInboxBinding:
    """Trusted host selection, not account proof or a serializable permission.

    `account` must come from the selected native connection owner's verified
    tuple. `me` is the authenticated user's mailbox resource, never a guessed
    email-to-subject mapping. Topics/projection changes require a new generation.
    """
    account: VerifiedConnectorAccount
    topics: tuple[str, ...]
    source_id: str

    def __post_init__(self):
        if type(self.account) is not VerifiedConnectorAccount:
            raise GmailObservationError("verified_account_required")
        for value in (self.account.issuer, self.account.strategy_id, self.account.subject):
            _text(value, 1024)
        _text(self.source_id, 200)
        if type(self.topics) is not tuple or not 1 <= len(self.topics) <= 16:
            raise GmailObservationError("invalid_source_topics")
        for topic in self.topics:
            _text(topic)
        if len(set(self.topics)) != len(self.topics):
            raise GmailObservationError("invalid_source_topics")
        if len(_json(self.principal()).encode()) > 1024:
            raise GmailObservationError("invalid_source_principal")

    def principal(self) -> dict[str, str]:
        return {"kind": "connector_verified_account", "issuer": self.account.issuer,
                "strategy_id": self.account.strategy_id, "subject": self.account.subject}

    def identity(self) -> dict[str, Any]:
        return {"provider": "google.gmail", "principal": self.principal(),
                "scope": {"kind": "gmail_mailbox", "id": "me"}}

    def digest(self) -> str:
        return hashlib.sha256(_json({"identity": self.identity(), "topics": self.topics,
            "source_id": self.source_id, "projection": PROJECTION}).encode()).hexdigest()


@dataclass(frozen=True)
class GmailObservationPage:
    items: tuple[dict[str, Any], ...] = field(repr=False)
    cursor: str = field(repr=False)
    page_index: int
    final: bool
    coverage: str | None = None
    messages_examined: int = 0
    observed: dict[str, str] | None = None


class GmailInboxObservation:
    """Read one native advance's worth of data, never commit or retry it.

    At most one history record and 100 message candidates are processed per
    call. Oversized records continue by re-reading that exact provider page,
    checked against its record digest and offset in the native-owned cursor.
    No retained item cache, seen set or cursor file exists here.
    """
    def __init__(self, operations: GmailReadOperations, binding: GmailInboxBinding, *,
                 check_current: Callable[[], Awaitable[VerifiedConnectorAccount]]):
        if type(binding) is not GmailInboxBinding or not callable(check_current):
            raise GmailObservationError("invalid_host_binding")
        self._operations, self._binding, self._check_current = operations, binding, check_current

    async def _current(self) -> None:
        # Exceptions from the owner remain fixed and value-free at this adapter
        # boundary. They are never treated as an empty successful observation.
        failed = cancelled = False
        try:
            actual = await self._check_current()
        except asyncio.CancelledError:
            cancelled = True
        except Exception:
            failed = True
        if cancelled:
            raise asyncio.CancelledError() from None
        if failed:
            raise GmailObservationError("connection_admission_unconfirmed") from None
        if type(actual) is not VerifiedConnectorAccount or actual != self._binding.account:
            raise GmailObservationError("principal_changed")

    async def _read(self, operation: str, **arguments: Any) -> dict[str, Any]:
        await self._current()
        failure = None
        try:
            result = await getattr(self._operations, operation)(**arguments)
        except GmailError as error:
            failure = error
        # Check the connection even after a provider refusal. A replaced
        # account's 404 must not become a gap for the previously selected one.
        await self._current()
        if failure is not None:
            raise failure from None
        return result

    def _page(self, items: tuple[dict[str, Any], ...], cursor: str, index: int, final: bool,
              *, coverage: str | None = None, examined: int = 0) -> GmailObservationPage:
        # These are final serialized output bounds, in addition to the borrowed
        # Gmail transport's response bound. A provider record is not an item cap.
        # Only a settled response to the history query confirms a delta. The
        # profile checkpoint baseline is not a delta query, and an interim page
        # must not claim completed coverage or a settled health observation.
        observed = {"kind": "delta"} if final and coverage is None else None
        bodies = [len(item["body"].encode("utf-8")) for item in items]
        if (len(items) > MAX_ITEMS or any(size > MAX_BODY_BYTES for size in bodies)
                or sum(bodies) > MAX_PAGE_BYTES
                or any(len(item["item_id"].encode()) > 256 or len(item["version"].encode()) > 256
                       for item in items)
                or len(_json({"items": items, "cursor": cursor, "page_index": index,
                              "final": final, "coverage": coverage,
                              "observed": observed, "messages_examined": examined}).encode()) > MAX_PAGE_BYTES):
            raise GmailObservationError("observation_page_too_large")
        return GmailObservationPage(items, cursor, index, final, coverage, examined, observed)

    def _cursor(self, **state: Any) -> str:
        if state.get("kind") == "page" and not 1 <= state["index"] <= 0xFFFFFFFF:
            raise GmailObservationError("page_index_exhausted")
        token = _json({"v": 1, "binding": self._binding.digest(), **state})
        if len(token.encode()) > MAX_CURSOR_BYTES:
            raise GmailObservationError("cursor_too_large")
        return token

    def _decode(self, token: str) -> dict[str, Any]:
        invalid = False
        try:
            if type(token) is not str or len(token.encode()) > MAX_CURSOR_BYTES:
                raise ValueError
            value = json.loads(token, object_pairs_hook=_object)
            if (type(value) is not dict or value.get("v") != 1 or type(value["v"]) is not int
                    or value.get("binding") != self._binding.digest()):
                raise ValueError
            common = {"v", "binding", "kind", "history_id"}
            if value.get("kind") == "checkpoint":
                if set(value) != common:
                    raise ValueError
            elif value.get("kind") == "page":
                if set(value) != common | {"page_token", "resume", "index"}:
                    raise ValueError
                if value["page_token"] is not None:
                    _text(value["page_token"], MAX_CURSOR_BYTES)
                if type(value["index"]) is not int or not 1 <= value["index"] <= 0xFFFFFFFF:
                    raise ValueError
                resume = value["resume"]
                if resume is not None:
                    if (type(resume) is not dict or set(resume) != {"record", "digest", "offset"}
                            or type(resume["offset"]) is not int or not 1 <= resume["offset"] <= 2_097_152
                            or type(resume["digest"]) is not str or len(resume["digest"]) != 64
                            or any(c not in "0123456789abcdef" for c in resume["digest"])):
                        raise ValueError
                    _history(resume["record"])
            else:
                raise ValueError
            _history(value["history_id"])
        except Exception:
            invalid = True
        if invalid:
            raise GmailObservationError("invalid_cursor") from None
        return value

    async def collect(self, cursor: str | None) -> GmailObservationPage:
        """None takes a delta baseline, with no historical item replay.

        A caller retains this exact page until Meta's advance is known, then
        uses the native returned cursor/version. No settled health, receipt or
        delivery claim is made by returning this page.
        """
        if cursor is None:
            profile = await self._read("get_profile")
            history_id = _history(profile.get("historyId"))
            return self._page((), self._cursor(kind="checkpoint", history_id=history_id),
                              0, True, coverage="complete")
        state = self._decode(cursor)
        page_token = state.get("page_token")
        index = state.get("index", 0)
        try:
            page = await self._read("history_page", start_history_id=state["history_id"],
                                   page_token=page_token, max_results=1,
                                   history_types=("messageAdded",))
        except GmailError as error:
            if error.kind == FailureKind.NOT_FOUND and error.status_code == 404:
                raise GmailRebaselineRequired() from None
            raise
        head = _history(page.get("historyId"))
        if int(head) < int(state["history_id"]):
            raise GmailObservationError("history_regressed")
        records = page.get("history", [])
        next_token = page.get("nextPageToken")
        if type(records) is not list or len(records) > 1:
            raise GmailObservationError("invalid_history_page")
        if next_token is not None:
            _text(next_token, MAX_CURSOR_BYTES)
            if next_token == page_token:
                raise GmailObservationError("history_page_stalled")
        resume = state.get("resume")
        items: list[dict[str, Any]] = []
        examined = 0
        if not records:
            if resume is not None:
                raise GmailObservationError("history_page_changed")
        else:
            record = records[0]
            if type(record) is not dict:
                raise GmailObservationError("invalid_history_page")
            record_id = _history(record.get("id"))
            if not int(state["history_id"]) < int(record_id) <= int(head):
                raise GmailObservationError("history_regressed")
            additions = record.get("messagesAdded", [])
            if type(additions) is not list:
                raise GmailObservationError("invalid_history_page")
            messages: dict[str, dict[str, Any]] = {}
            for addition in additions:
                message = addition.get("message") if type(addition) is dict else None
                if type(message) is not dict:
                    raise GmailObservationError("invalid_history_page")
                message_id = _message_id(message.get("id"))
                selected = {"id": message_id, "threadId": _message_id(message.get("threadId"))}
                if "labelIds" in message:
                    selected["labelIds"] = self._labels(message["labelIds"])
                if message_id in messages and messages[message_id] != selected:
                    raise GmailObservationError("history_page_changed")
                messages[message_id] = selected
            ordered = [messages[key] for key in sorted(messages)]
            digest = hashlib.sha256(_json(ordered).encode()).hexdigest()
            offset = 0
            if resume is not None:
                if (resume["record"] != record_id or resume["digest"] != digest
                        or resume["offset"] >= len(ordered)):
                    raise GmailObservationError("history_page_changed")
                offset = resume["offset"]
            for message in ordered[offset:offset + MAX_ITEMS]:
                examined += 1
                labels = message.get("labelIds")
                if labels is None:
                    missing = False
                    try:
                        current = await self._read("get_message", message_id=message["id"], format="minimal")
                    except GmailError as error:
                        if error.kind != FailureKind.NOT_FOUND or error.status_code != 404:
                            raise
                        missing = True
                    if missing:
                        # A deletion between history and label resolution cannot
                        # establish INBOX membership. Do not invent a deletion,
                        # skip it with healthy coverage, or advance past it.
                        raise GmailObservationGap() from None
                    if current.get("id") != message["id"] or current.get("threadId") != message["threadId"]:
                        raise GmailObservationError("message_binding_changed")
                    labels = self._labels(current.get("labelIds"))
                if "INBOX" in labels and not {"SENT", "DRAFT"}.intersection(labels):
                    items.append({"item_id": message["id"], "version": f"{PROJECTION}:{record_id}",
                        "stream": "", "change": "upsert", "topics": list(self._binding.topics),
                        "body": _json({"provider": "google.gmail", "message_id": message["id"],
                                       "thread_id": message["threadId"]})})
            if offset + examined < len(ordered):
                token = self._cursor(kind="page", history_id=state["history_id"], page_token=page_token,
                    resume={"record": record_id, "digest": digest, "offset": offset + examined}, index=index + 1)
                return self._page(tuple(items), token, index, False, examined=examined)
        if next_token is not None:
            token = self._cursor(kind="page", history_id=state["history_id"], page_token=next_token,
                                 resume=None, index=index + 1)
            return self._page(tuple(items), token, index, False, examined=examined)
        token = self._cursor(kind="checkpoint", history_id=head)
        return self._page(tuple(items), token, index, True, examined=examined)

    @staticmethod
    def _labels(value: Any) -> list[str]:
        if type(value) is not list or len(value) > 1024:
            raise GmailObservationError("message_labels_unavailable")
        return sorted({_text(label, 1024) for label in value})
