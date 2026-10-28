"""Durable sender selection, extracted from the paired host's existing SQLite owner.

Selection survives reopening the same database; live implementations do not.
The host owns sender eligibility, provider credentials, lifecycle composition,
egress outcomes and generation rollback. This module grants no request authority.
"""

from __future__ import annotations

import asyncio
from collections import defaultdict
from collections.abc import Callable, Mapping
from contextlib import closing
from dataclasses import dataclass, field
import inspect
import os
from pathlib import Path
import sqlite3
import time
from typing import Any
from weakref import WeakKeyDictionary


async def _await(value: Any) -> Any:
    return await value if inspect.isawaitable(value) else value


def _checked_thread_id(value: str | None) -> str | None:
    """Validate an opaque host-selected target without normalizing its identity."""
    if value is None:
        return None
    if not isinstance(value, str) or not value.strip():
        raise ValueError("thread_id must be nonblank text")
    try:
        encoded = value.encode("utf-8")
    except UnicodeError:
        raise ValueError("thread_id must be valid UTF-8") from None
    if len(encoded) > 1024 or any(ord(char) < 32 or ord(char) == 127 for char in value):
        raise ValueError("thread_id exceeds its bounds")
    return value


def _thread_arguments(sender: Callable[..., Any], thread_id: str | None) -> dict[str, str]:
    """Require explicit target support; a catch-all wrapper can silently drop it."""
    thread_id = _checked_thread_id(thread_id)
    if thread_id is None:
        return {}
    try:
        parameter = inspect.signature(sender, follow_wrapped=False).parameters.get("thread_id")
    except (TypeError, ValueError):
        parameter = None
    if parameter is None or parameter.kind not in (
        inspect.Parameter.KEYWORD_ONLY, inspect.Parameter.POSITIONAL_OR_KEYWORD
    ):
        raise ValueError("sender does not explicitly support thread_id")
    return {"thread_id": thread_id}


def _output_arguments(sender: Callable[..., Any], output_id: str | None) -> dict[str, str]:
    """Committed identity is opt-in; a catch-all signature is not support."""
    if output_id is None:
        return {}
    if type(output_id) is not str or not output_id or len(output_id) > 128:
        raise ValueError("invalid committed output identity")
    try:
        parameter = inspect.signature(sender, follow_wrapped=False).parameters.get("output_id")
    except (TypeError, ValueError):
        parameter = None
    if parameter is None or parameter.kind not in (
        inspect.Parameter.KEYWORD_ONLY, inspect.Parameter.POSITIONAL_OR_KEYWORD
    ):
        raise ValueError("sender does not explicitly support output_id")
    return {"output_id": output_id}


def _fsync_file(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _fsync_directory(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


@dataclass(frozen=True, eq=False)
class SenderActivation:
    """An exact, instance-issued handle, not a reconstructible owner selection."""

    sender_id: str
    previous_owner_id: str | None
    to_owner_id: str


@dataclass
class _SenderCandidate:
    sender: Any
    previous_attachment: object | None
    attachment: object = field(default_factory=object)
    selection_committed: bool = False
    published: bool = False


@dataclass
class _SenderLifecycle:
    lock: asyncio.Lock = field(default_factory=asyncio.Lock)
    dispatch_revision: int = 0
    deactivating: object | None = None


@dataclass(frozen=True, eq=False)
class SenderDispatchSelection:
    """Borrowed local selection, not permission, readiness or delivery proof."""

    sender_id: str
    owner_id: str
    sender: Callable[..., Any] = field(repr=False)
    _authority: object = field(repr=False)
    _attachment: object = field(repr=False)
    _dispatch_revision: int = field(repr=False)


class SenderAuthority:
    """One durable active owner for a stable provider sender identity."""

    def __init__(self, *, path: Path) -> None:
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._senders: dict[tuple[str, str], Any] = {}
        self._attachments: dict[str, object] = {}
        # Neither the keys nor predecessor attachment tokens retain handles
        # after callers release them. Active implementations remain attached.
        self._candidates: WeakKeyDictionary[
            SenderActivation, _SenderCandidate
        ] = WeakKeyDictionary()
        # Lifecycle work can affect an implementation even if it later fails.
        # Only that sender's borrowed selections and lifecycle are affected.
        self._lifecycles: dict[str, _SenderLifecycle] = defaultdict(_SenderLifecycle)
        created = not self.path.exists()
        with closing(self._connect()) as connection, connection:
            connection.execute(
                """
                CREATE TABLE IF NOT EXISTS sender_authority (
                    sender_id TEXT PRIMARY KEY,
                    owner_id TEXT NOT NULL,
                    updated_at_ms INTEGER NOT NULL
                )
                """
            )
        if created:
            _fsync_file(self.path)
            _fsync_directory(self.path.parent)

    def _connect(self) -> sqlite3.Connection:
        connection = sqlite3.connect(self.path)
        try:
            connection.execute("PRAGMA synchronous = FULL")
        except BaseException:
            # Setup failed before the caller could acquire this connection.
            try:
                connection.close()
            except BaseException:
                pass  # Preserve the original setup failure.
            raise
        return connection

    @staticmethod
    def _identity(value: object, *, field: str) -> str:
        if not isinstance(value, str) or not value:
            raise ValueError(f"{field} must be nonempty text")
        return value

    @staticmethod
    def _sender_implementation(sender: object) -> bool:
        return callable(sender) or callable(getattr(sender, "send", None))

    def attach_active(self, *, sender_id: str, owner_id: str, sender: object) -> None:
        """Attach an implementation for an already-selected or new stable owner."""

        sender_id = self._identity(sender_id, field="sender_id")
        owner_id = self._identity(owner_id, field="owner_id")
        if not self._sender_implementation(sender):
            raise TypeError("sender has no dispatch implementation")
        if self._lifecycles[sender_id].deactivating is sender:
            raise ValueError("sender implementation is still deactivating")
        with closing(self._connect()) as connection, connection:
            existing = connection.execute(
                "SELECT owner_id FROM sender_authority WHERE sender_id = ?",
                (sender_id,),
            ).fetchone()
            if existing is not None and existing[0] != owner_id:
                raise ValueError("stable sender already has a different active owner")
            connection.execute(
                """
                INSERT INTO sender_authority (sender_id, owner_id, updated_at_ms)
                VALUES (?, ?, ?)
                ON CONFLICT(sender_id) DO UPDATE SET updated_at_ms = excluded.updated_at_ms
                """,
                (sender_id, owner_id, int(time.time_ns() // 1_000_000)),
            )
            connection.commit()
        _fsync_file(self.path)
        self._publish(sender_id, owner_id, sender, object())

    def _publish(self, sender_id: str, owner_id: str, sender: Any, attachment: object) -> None:
        # A historical owner becoming selected again must not revive its old
        # implementation before that replacement has completed activation.
        for key in [key for key in self._senders if key[0] == sender_id]:
            del self._senders[key]
        self._senders[(sender_id, owner_id)] = sender
        self._attachments[sender_id] = attachment

    def active_owner(self, sender_id: str) -> str | None:
        with closing(self._connect()) as connection, connection:
            row = connection.execute(
                "SELECT owner_id FROM sender_authority WHERE sender_id = ?",
                (sender_id,),
            ).fetchone()
        return str(row[0]) if row is not None else None

    def owner_count(self, sender_id: str) -> int:
        with closing(self._connect()) as connection, connection:
            row = connection.execute(
                "SELECT COUNT(*) FROM sender_authority WHERE sender_id = ?",
                (sender_id,),
            ).fetchone()
        return int(row[0]) if row is not None else 0

    def has_sender(self, sender_id: str) -> bool:
        owner = self.active_owner(sender_id)
        return owner is not None and (sender_id, owner) in self._senders

    def stage_activation(
        self,
        *,
        sender_id: str,
        to_owner_id: str,
        sender: object,
    ) -> SenderActivation:
        sender_id = self._identity(sender_id, field="sender_id")
        to_owner_id = self._identity(to_owner_id, field="to_owner_id")
        if not all(callable(getattr(sender, name, None)) for name in ("activate", "deactivate")):
            raise TypeError("candidate sender lifecycle is incomplete")
        if not self._sender_implementation(sender):
            raise TypeError("candidate sender dispatch implementation is incomplete")
        previous_owner_id = self.active_owner(sender_id)
        activation = SenderActivation(
            sender_id=sender_id,
            previous_owner_id=previous_owner_id,
            to_owner_id=to_owner_id,
        )
        self._candidates[activation] = _SenderCandidate(
            sender=sender,
            previous_attachment=self._attachments.get(sender_id),
        )
        return activation

    def _candidate(self, activation: SenderActivation) -> _SenderCandidate:
        if type(activation) is not SenderActivation or activation not in self._candidates:
            raise ValueError("sender activation was not issued by this authority instance")
        candidate = self._candidates[activation]
        attachment = candidate.attachment if candidate.published else candidate.previous_attachment
        if self._attachments.get(activation.sender_id) is not attachment:
            raise ValueError("sender attachment changed after candidate staging")
        return candidate

    async def commit_activation(self, activation: SenderActivation) -> None:
        self._candidate(activation)
        lifecycle = self._lifecycles[activation.sender_id]
        async with lifecycle.lock:
            candidate = self._candidate(activation)
            owner = self.active_owner(activation.sender_id)
            expected_owner = (
                activation.to_owner_id
                if candidate.selection_committed
                else activation.previous_owner_id
            )
            if owner != expected_owner:
                raise ValueError("sender authority changed after candidate staging")
            lifecycle.dispatch_revision += 1
            await _await(candidate.sender.activate())
            # attach_active is synchronous and can run while activate awaits.
            # Its new attachment supersedes this handle even for the same owner.
            self._candidate(activation)
            with closing(self._connect()) as connection, connection:
                connection.execute("BEGIN IMMEDIATE")
                if expected_owner is None:
                    cursor = connection.execute(
                        """
                        INSERT INTO sender_authority (sender_id, owner_id, updated_at_ms)
                        VALUES (?, ?, ?)
                        ON CONFLICT(sender_id) DO NOTHING
                        """,
                        (
                            activation.sender_id,
                            activation.to_owner_id,
                            int(time.time_ns() // 1_000_000),
                        ),
                    )
                elif expected_owner == activation.to_owner_id:
                    # Reopening and exact replay preserve the existing row,
                    # while still checking its owner inside the transaction.
                    cursor = connection.execute(
                        """
                        UPDATE sender_authority SET owner_id = owner_id
                        WHERE sender_id = ? AND owner_id = ?
                        """,
                        (activation.sender_id, expected_owner),
                    )
                else:
                    cursor = connection.execute(
                        """
                        UPDATE sender_authority SET owner_id = ?, updated_at_ms = ?
                        WHERE sender_id = ? AND owner_id = ?
                        """,
                        (
                            activation.to_owner_id,
                            int(time.time_ns() // 1_000_000),
                            activation.sender_id,
                            expected_owner,
                        ),
                    )
                if cursor.rowcount != 1:
                    connection.rollback()
                else:
                    connection.commit()
                    # A subsequent fsync error does not roll this commit back.
                    # Only this exact handle may retry its selected candidate.
                    candidate.selection_committed = True
            if cursor.rowcount != 1:
                await self._deactivate(activation.sender_id, candidate.sender)
                raise ValueError("sender authority changed during activation")
            _fsync_file(self.path)
            self._publish(
                activation.sender_id, activation.to_owner_id, candidate.sender, candidate.attachment
            )
            candidate.published = True

    async def close_activation(self, activation: SenderActivation) -> None:
        self._candidate(activation)
        lifecycle = self._lifecycles[activation.sender_id]
        async with lifecycle.lock:
            candidate = self._candidate(activation)
            owner = self.active_owner(activation.sender_id)
            expected_owner = (
                activation.to_owner_id
                if candidate.selection_committed
                else activation.previous_owner_id
            )
            if owner != expected_owner:
                raise ValueError("sender authority changed after candidate activation")
            if candidate.selection_committed:
                lifecycle.dispatch_revision += 1
                await self._deactivate(activation.sender_id, candidate.sender)

    async def _deactivate(self, sender_id: str, sender: Any) -> None:
        # Both close and losing an activation CAS can deactivate an attached
        # implementation. Fence only its route before cleanup can await or fail.
        for key in [
            key for key, attached in self._senders.items()
            if key[0] == sender_id and attached is sender
        ]:
            del self._senders[key]
        # Retain the attachment token for exact close retry or reactivation.
        lifecycle = self._lifecycles[sender_id]
        lifecycle.deactivating = sender
        try:
            await _await(sender.deactivate())
        finally:
            # A different synchronous attachment may have replaced this route
            # during cleanup. Never remove it after the await.
            lifecycle.deactivating = None

    async def send(self, *, sender_id: str, recipient: str, parts: tuple[str, ...]) -> Any:
        owner = self.active_owner(sender_id)
        if owner is None:
            raise ValueError("stable sender has no active owner")
        sender = self._senders.get((sender_id, owner))
        if sender is None:
            raise ValueError("active sender implementation is not attached")
        return await _await(sender.send(recipient, parts))

    def is_attached(self, sender_id: str) -> bool:
        """Whether the durable owner has a live implementation in this instance.

        Hosts can skip recovery while detached without spending retry attempts.
        Attachment is not evidence that the sender is open or has delivered.
        """

        owner = self.active_owner(sender_id)
        return owner is not None and (sender_id, owner) in self._senders

    def supports_choices(self, sender_id: str) -> bool:
        """Early capability hint only; dispatch must validate its borrowed sender."""
        owner = self.active_owner(sender_id)
        sender = self._senders.get((sender_id, owner))
        return callable(sender) and getattr(sender, "supports_choices", False) is True

    async def dispatch(
        self,
        *,
        sender_id: str,
        recipient: str,
        part_index: int,
        part: Mapping[str, Any],
        thread_id: str | None = None,
        output_id: str | None = None,
    ) -> Any:
        selection = await self.select_dispatch(sender_id=sender_id)
        if part.get("choices") and getattr(selection.sender, "supports_choices", False) is not True:
            raise ValueError("sender does not support choices")
        return await _await(
            selection.sender(
                sender_id=sender_id,
                recipient=recipient,
                part_index=part_index,
                part=part,
                **_thread_arguments(selection.sender, thread_id),
                **_output_arguments(selection.sender, output_id),
            )
        )

    async def select_dispatch(self, *, sender_id: str) -> SenderDispatchSelection:
        """Wait for local selection before the caller's current authorization.

        Callers that await again must recheck ``is_current_dispatch`` before
        provider entry. No lock is held across host policy or provider calls.
        """
        lifecycle = self._lifecycles[sender_id]
        async with lifecycle.lock:
            owner = self.active_owner(sender_id)
            if owner is None:
                raise ValueError("stable sender has no active owner")
            sender = self._senders.get((sender_id, owner))
            if sender is None:
                raise ValueError("active sender implementation is not attached")
            if not callable(sender):
                raise TypeError("active sender is not ProactiveEgress-callable")
            return SenderDispatchSelection(
                sender_id, owner, sender, self, self._attachments[sender_id],
                lifecycle.dispatch_revision,
            )

    def is_current_dispatch(self, selection: SenderDispatchSelection) -> bool:
        """Synchronous local custody check; never a permission or distributed fence."""
        if type(selection) is not SenderDispatchSelection or selection._authority is not self:
            return False
        lifecycle = self._lifecycles.get(selection.sender_id)
        return (
            lifecycle is not None
            and not lifecycle.lock.locked()
            and selection._dispatch_revision == lifecycle.dispatch_revision
            and self._attachments.get(selection.sender_id) is selection._attachment
            and self.active_owner(selection.sender_id) == selection.owner_id
            and self._senders.get((selection.sender_id, selection.owner_id)) is selection.sender
        )
