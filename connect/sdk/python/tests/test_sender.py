"""Existing SQLite selection semantics, including reopened and competing owners."""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable
from contextlib import closing
import copy
import gc
from pathlib import Path
import sqlite3
from typing import Any
import weakref

import pytest

from meerkat_toolkit.frontend import sender as sender_module
from meerkat_toolkit.frontend.sender import SenderActivation, SenderAuthority


SENDER = "provider:primary:sender"


class Candidate:
    def __init__(
        self,
        *,
        activate: Callable[[], Awaitable[None]] | None = None,
        deactivate: Callable[[], Awaitable[None]] | None = None,
        dispatch: Callable[[], Awaitable[None]] | None = None,
    ) -> None:
        self.lifecycle: list[str] = []
        self.calls: list[dict[str, Any]] = []
        self.on_activate = activate
        self.on_deactivate = deactivate
        self.on_dispatch = dispatch

    async def activate(self) -> None:
        self.lifecycle.append("activate")
        if self.on_activate is not None:
            await self.on_activate()

    async def deactivate(self) -> None:
        self.lifecycle.append("deactivate")
        if self.on_deactivate is not None:
            await self.on_deactivate()

    async def send(self, recipient: str, parts: tuple[str, ...]) -> dict[str, str]:
        return await self(recipient=recipient, parts=parts)

    async def __call__(self, **kwargs: Any) -> dict[str, str]:
        self.calls.append(kwargs)
        if self.on_dispatch is not None:
            await self.on_dispatch()
        return {"outcome": "sent", "provider_message_id": "fixture-result"}


@pytest.mark.asyncio
async def test_connections_close_after_selection_reads_commits_and_rollback(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    original_connect = sqlite3.connect
    opened: list[sqlite3.Connection] = []

    def connect(*args: Any, **kwargs: Any) -> sqlite3.Connection:
        connection = original_connect(*args, **kwargs)
        opened.append(connection)
        return connection

    def assert_closed() -> None:
        assert opened
        for connection in opened:
            with pytest.raises(sqlite3.ProgrammingError, match="closed database"):
                connection.execute("SELECT 1")

    monkeypatch.setattr(sender_module.sqlite3, "connect", connect)
    path = tmp_path / "sender-authority.sqlite3"
    try:
        authority = SenderAuthority(path=path)
        assert_closed()
        authority.attach_active(sender_id=SENDER, owner_id="old", sender=Candidate())
        assert_closed()
        assert authority.active_owner(SENDER) == "old"
        assert authority.owner_count(SENDER) == 1
        assert_closed()
        activation = authority.stage_activation(
            sender_id=SENDER, to_owner_id="new", sender=Candidate(),
        )
        await authority.commit_activation(activation)
        assert rows(path)[0][:2] == (SENDER, "new")
        assert_closed()
        await authority.close_activation(activation)

        with closing(original_connect(path)) as connection, connection:
            connection.execute(
                "CREATE TRIGGER reject_blocked BEFORE UPDATE ON sender_authority "
                "WHEN NEW.owner_id = 'blocked' BEGIN SELECT RAISE(ABORT, 'blocked'); END"
            )
        before = rows(path)
        failed = authority.stage_activation(
            sender_id=SENDER, to_owner_id="blocked", sender=Candidate(),
        )
        with pytest.raises(sqlite3.IntegrityError, match="blocked"):
            await authority.commit_activation(failed)
        assert rows(path) == before
        assert_closed()
        await authority.close_activation(failed)
    finally:
        for connection in opened:
            connection.close()


@pytest.mark.parametrize("phase,close_fails", [
    ("pragma", False), ("pragma", True), ("schema", False), ("schema", True),
])
def test_initialization_failure_closes_real_connection_with_expected_error_chain(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, phase: str, close_fails: bool,
) -> None:
    original_connect = sqlite3.connect
    opened: list[sqlite3.Connection] = []
    errors: list[sqlite3.DatabaseError] = []

    class Connection(sqlite3.Connection):
        def execute(self, *args: Any, **kwargs: Any) -> sqlite3.Cursor:
            try:
                return super().execute(*args, **kwargs)
            except sqlite3.DatabaseError as error:
                errors.append(error)
                raise

        def close(self) -> None:
            super().close()
            if close_fails:
                raise OSError("controlled close failure after actual closure")

    def connect(*args: Any, **kwargs: Any) -> sqlite3.Connection:
        connection = original_connect(*args, **kwargs, factory=Connection)
        opened.append(connection)
        denied = sqlite3.SQLITE_PRAGMA if phase == "pragma" else sqlite3.SQLITE_CREATE_TABLE
        connection.set_authorizer(
            lambda action, *_: sqlite3.SQLITE_DENY if action == denied else sqlite3.SQLITE_OK
        )
        return connection

    monkeypatch.setattr(sender_module.sqlite3, "connect", connect)
    try:
        # Raw PRAGMA setup preserves its original failure. The constructor's
        # outer closing scope keeps normal Python exception chaining instead.
        constructor_close_failure = phase == "schema" and close_fails
        expected = OSError if constructor_close_failure else sqlite3.DatabaseError
        with pytest.raises(expected) as caught:
            SenderAuthority(path=tmp_path / "sender-authority.sqlite3")
        assert len(opened) == 1
        if constructor_close_failure:
            assert caught.value.__context__ is errors[0]
        else:
            assert caught.value is errors[0]
        with pytest.raises(sqlite3.ProgrammingError, match="closed database"):
            opened[0].execute("SELECT 1")
    finally:
        for connection in opened:
            sqlite3.Connection.close(connection)


def rows(path: Path) -> list[tuple[str, str, int]]:
    with closing(sqlite3.connect(path)) as connection:
        return connection.execute(
            "SELECT sender_id, owner_id, updated_at_ms FROM sender_authority ORDER BY sender_id"
        ).fetchall()


def existing_database(path: Path) -> None:
    # An actual pre-extraction database, not one manufactured by the new owner.
    with closing(sqlite3.connect(path)) as connection, connection:
        connection.execute(
            "CREATE TABLE sender_authority (sender_id TEXT PRIMARY KEY, "
            "owner_id TEXT NOT NULL, updated_at_ms INTEGER NOT NULL)"
        )
        connection.execute("INSERT INTO sender_authority VALUES (?, 'old', 123)", (SENDER,))


async def dispatch(authority: SenderAuthority) -> dict[str, str]:
    return await authority.dispatch(
        sender_id=SENDER, recipient="recipient", part_index=0, part={"text": "hello"}
    )


@pytest.mark.asyncio
async def test_existing_selection_reopens_detached_then_replays_and_closes(tmp_path: Path) -> None:
    path = tmp_path / "sender-authority.sqlite3"
    existing_database(path)
    original = rows(path)
    authority = SenderAuthority(path=path)
    assert rows(path) == original
    assert authority.active_owner(SENDER) == "old"
    assert not authority.is_attached(SENDER)
    with pytest.raises(ValueError, match="not attached"):
        await dispatch(authority)
    with pytest.raises(ValueError, match="different active owner"):
        authority.attach_active(sender_id=SENDER, owner_id="other", sender=Candidate())
    assert rows(path) == original

    candidate = Candidate()
    activation = authority.stage_activation(sender_id=SENDER, to_owner_id="new", sender=candidate)
    assert rows(path) == original
    assert candidate.lifecycle == []
    await authority.commit_activation(activation)
    selected = rows(path)
    assert len(selected) == 1 and selected[0][:2] == (SENDER, "new")
    await authority.commit_activation(activation)
    assert rows(path) == selected
    # Existing replay semantics call activate again, without rewriting selection.
    assert candidate.lifecycle == ["activate", "activate"]
    assert (await dispatch(authority))["outcome"] == "sent"
    await authority.close_activation(activation)
    assert rows(path) == selected
    assert candidate.lifecycle == ["activate", "activate", "deactivate"]
    assert not authority.is_attached(SENDER)

    del authority
    reopened = SenderAuthority(path=path)
    assert rows(path) == selected
    assert not reopened.has_sender(SENDER)
    with pytest.raises(ValueError, match="not attached"):
        await dispatch(reopened)
    replacement = Candidate()
    replay = reopened.stage_activation(sender_id=SENDER, to_owner_id="new", sender=replacement)
    await reopened.commit_activation(replay)
    assert rows(path) == selected
    assert replacement.lifecycle == ["activate"]
    await dispatch(reopened)
    assert len(replacement.calls) == 1
    with closing(sqlite3.connect(path)) as connection:
        assert connection.execute(
            "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name"
        ).fetchall() == [("sender_authority",)]
        assert connection.execute("PRAGMA table_info(sender_authority)").fetchall() == [
            (0, "sender_id", "TEXT", 0, None, 1),
            (1, "owner_id", "TEXT", 1, None, 0),
            (2, "updated_at_ms", "INTEGER", 1, None, 0),
        ]


@pytest.mark.asyncio
async def test_activation_failure_keeps_original_selection_and_dispatch(tmp_path: Path) -> None:
    path = tmp_path / "sender-authority.sqlite3"
    authority = SenderAuthority(path=path)
    old = Candidate()
    authority.attach_active(sender_id=SENDER, owner_id="old", sender=old)
    original = rows(path)

    async def refuse() -> None:
        raise RuntimeError("candidate cannot open")

    candidate = Candidate(activate=refuse)
    activation = authority.stage_activation(sender_id=SENDER, to_owner_id="new", sender=candidate)
    with pytest.raises(RuntimeError, match="candidate cannot open"):
        await authority.commit_activation(activation)
    assert rows(path) == original
    assert SenderAuthority(path=path).active_owner(SENDER) == "old"
    await authority.close_activation(activation)
    assert candidate.lifecycle == ["activate"]
    await dispatch(authority)
    assert len(old.calls) == 1 and candidate.calls == []


@pytest.mark.asyncio
@pytest.mark.parametrize("previous", [None, "old"])
@pytest.mark.parametrize("switch_during_activation", [False, True])
async def test_competing_instances_cannot_overwrite_winning_selection(
    tmp_path: Path, previous: str | None, switch_during_activation: bool
) -> None:
    path = tmp_path / "sender-authority.sqlite3"
    losing_authority = SenderAuthority(path=path)
    if previous is not None:
        losing_authority.attach_active(sender_id=SENDER, owner_id=previous, sender=Candidate())
    winning_authority = SenderAuthority(path=path)
    entered = asyncio.Event()
    resume = asyncio.Event()

    async def pause() -> None:
        entered.set()
        await resume.wait()

    loser = Candidate(activate=pause if switch_during_activation else None)
    winner = Candidate()
    losing = losing_authority.stage_activation(sender_id=SENDER, to_owner_id="loser", sender=loser)
    winning = winning_authority.stage_activation(
        sender_id=SENDER, to_owner_id="winner", sender=winner
    )
    pending = None
    try:
        if switch_during_activation:
            pending = asyncio.create_task(losing_authority.commit_activation(losing))
            await asyncio.wait_for(entered.wait(), timeout=2)
        await winning_authority.commit_activation(winning)
        selected = rows(path)
        if switch_during_activation:
            resume.set()
            with pytest.raises(ValueError, match="changed during activation"):
                await asyncio.wait_for(pending, timeout=2)
        else:
            with pytest.raises(ValueError, match="changed after candidate staging"):
                await losing_authority.commit_activation(losing)
        assert rows(path) == selected
        assert SenderAuthority(path=path).active_owner(SENDER) == "winner"
        assert len(selected) == 1
        assert loser.lifecycle == (["activate", "deactivate"] if switch_during_activation else [])
        assert winner.lifecycle == ["activate"]
        with pytest.raises(ValueError, match="changed after candidate activation"):
            await losing_authority.close_activation(losing)
        assert rows(path) == selected
        assert winner.lifecycle == ["activate"]
        await dispatch(winning_authority)
        assert len(winner.calls) == 1 and loser.calls == []
    finally:
        if pending is not None and not pending.done():
            pending.cancel()
            await asyncio.gather(pending, return_exceptions=True)


@pytest.mark.asyncio
@pytest.mark.parametrize("previous", [None, "old"])
async def test_sqlite_abort_rolls_back_selection_without_claiming_lifecycle_compensation(
    tmp_path: Path, previous: str | None
) -> None:
    path = tmp_path / "sender-authority.sqlite3"
    authority = SenderAuthority(path=path)
    if previous is not None:
        authority.attach_active(sender_id=SENDER, owner_id=previous, sender=Candidate())
    original = rows(path)
    operation = "INSERT" if previous is None else "UPDATE"
    with closing(sqlite3.connect(path)) as connection, connection:
        connection.execute(
            f"CREATE TRIGGER refuse_selection AFTER {operation} ON sender_authority "
            "WHEN NEW.owner_id = 'new' "
            "BEGIN SELECT RAISE(ABORT, 'selection storage refused'); END"
        )
    candidate = Candidate()
    activation = authority.stage_activation(sender_id=SENDER, to_owner_id="new", sender=candidate)
    with pytest.raises(sqlite3.IntegrityError, match="selection storage refused"):
        await authority.commit_activation(activation)
    assert rows(path) == original
    assert SenderAuthority(path=path).active_owner(SENDER) == previous
    # The existing owner compensates CAS loss, not arbitrary database faults.
    assert candidate.lifecycle == ["activate"]
    with closing(sqlite3.connect(path)) as connection, connection:
        connection.execute("DROP TRIGGER refuse_selection")
    await authority.commit_activation(activation)
    assert SenderAuthority(path=path).active_owner(SENDER) == "new"
    assert len(rows(path)) == 1
    assert candidate.lifecycle == ["activate", "activate"]


@pytest.mark.asyncio
async def test_inflight_provider_call_does_not_block_switch_or_become_cancelled(
    tmp_path: Path,
) -> None:
    path = tmp_path / "sender-authority.sqlite3"
    authority = SenderAuthority(path=path)
    entered = asyncio.Event()
    resume = asyncio.Event()

    async def pause() -> None:
        entered.set()
        await resume.wait()

    old = Candidate(dispatch=pause)
    authority.attach_active(sender_id=SENDER, owner_id="old", sender=old)
    candidate = Candidate()
    activation = authority.stage_activation(sender_id=SENDER, to_owner_id="new", sender=candidate)
    pending = asyncio.create_task(dispatch(authority))
    try:
        await asyncio.wait_for(entered.wait(), timeout=2)
        await asyncio.wait_for(authority.commit_activation(activation), timeout=2)
        assert not pending.done()
        assert SenderAuthority(path=path).active_owner(SENDER) == "new"
        assert (await dispatch(authority))["outcome"] == "sent"
        resume.set()
        assert (await asyncio.wait_for(pending, timeout=2))["outcome"] == "sent"
        assert len(old.calls) == 1 and len(candidate.calls) == 1
    finally:
        if not pending.done():
            pending.cancel()
            await asyncio.gather(pending, return_exceptions=True)


@pytest.mark.asyncio
@pytest.mark.parametrize("cancel", [False, True])
async def test_same_owner_staging_and_failed_activation_keep_live_sender(
    tmp_path: Path, cancel: bool
) -> None:
    authority = SenderAuthority(path=tmp_path / "senders.sqlite3")
    old = Candidate()
    authority.attach_active(sender_id=SENDER, owner_id="same", sender=old)
    original = rows(authority.path)
    entered = asyncio.Event()

    async def fail() -> None:
        entered.set()
        if cancel:
            await asyncio.Event().wait()
        raise RuntimeError("activation refused")

    candidate = Candidate(activate=fail)
    handle = authority.stage_activation(sender_id=SENDER, to_owner_id="same", sender=candidate)
    await dispatch(authority)
    assert len(old.calls) == 1 and candidate.calls == []
    if cancel:
        pending = asyncio.create_task(authority.commit_activation(handle))
        await asyncio.wait_for(entered.wait(), timeout=2)
        pending.cancel()
        with pytest.raises(asyncio.CancelledError):
            await pending
    else:
        with pytest.raises(RuntimeError, match="activation refused"):
            await authority.commit_activation(handle)
    await authority.close_activation(handle)
    await dispatch(authority)
    assert rows(authority.path) == original
    assert len(old.calls) == 2 and candidate.calls == []
    assert candidate.lifecycle == ["activate"]


@pytest.mark.asyncio
@pytest.mark.parametrize("target", ["same", "new"])
@pytest.mark.parametrize("winner", [0, 1])
async def test_each_staged_handle_keeps_its_candidate_and_cannot_replace_winner(
    tmp_path: Path, target: str, winner: int
) -> None:
    authority = SenderAuthority(path=tmp_path / "senders.sqlite3")
    authority.attach_active(sender_id=SENDER, owner_id="same", sender=Candidate())
    candidates = [Candidate(), Candidate()]
    handles = [
        authority.stage_activation(sender_id=SENDER, to_owner_id=target, sender=candidate)
        for candidate in candidates
    ]
    assert handles[0] != handles[1]
    await authority.commit_activation(handles[winner])
    selected = rows(authority.path)
    loser = 1 - winner
    with pytest.raises(ValueError, match="attachment changed"):
        await authority.commit_activation(handles[loser])
    with pytest.raises(ValueError, match="attachment changed"):
        await authority.close_activation(handles[loser])
    await dispatch(authority)
    assert rows(authority.path) == selected
    assert candidates[winner].lifecycle == ["activate"]
    assert len(candidates[winner].calls) == 1
    assert candidates[loser].lifecycle == candidates[loser].calls == []


@pytest.mark.asyncio
async def test_foreign_and_reconstructed_handles_are_not_candidate_custody(tmp_path: Path) -> None:
    path = tmp_path / "senders.sqlite3"
    authority = SenderAuthority(path=path)
    candidate = Candidate()
    handle = authority.stage_activation(sender_id=SENDER, to_owner_id="new", sender=candidate)
    other = SenderAuthority(path=path)
    other_candidate = Candidate()
    other.stage_activation(sender_id=SENDER, to_owner_id="new", sender=other_candidate)
    reconstructed = SenderActivation(SENDER, None, "new")
    invalid_handles = [(other, handle), (authority, reconstructed), (authority, copy.copy(handle))]
    for owner, invalid in invalid_handles:
        with pytest.raises(ValueError, match="not issued"):
            await owner.commit_activation(invalid)
        with pytest.raises(ValueError, match="not issued"):
            await owner.close_activation(invalid)
    assert rows(path) == []
    assert candidate.lifecycle == other_candidate.lifecycle == []
    await authority.commit_activation(handle)
    assert candidate.lifecycle == ["activate"]


@pytest.mark.asyncio
async def test_superseded_committed_handle_cannot_reactivate_or_close_replacement(
    tmp_path: Path,
) -> None:
    authority = SenderAuthority(path=tmp_path / "senders.sqlite3")
    first, second = Candidate(), Candidate()
    old = authority.stage_activation(sender_id=SENDER, to_owner_id="same", sender=first)
    await authority.commit_activation(old)
    current = authority.stage_activation(sender_id=SENDER, to_owner_id="same", sender=second)
    await authority.commit_activation(current)
    with pytest.raises(ValueError, match="attachment changed"):
        await authority.commit_activation(old)
    with pytest.raises(ValueError, match="attachment changed"):
        await authority.close_activation(old)
    await dispatch(authority)
    assert first.lifecycle == second.lifecycle == ["activate"]
    assert first.calls == [] and len(second.calls) == 1
    await authority.close_activation(current)
    assert second.lifecycle == ["activate", "deactivate"]


@pytest.mark.asyncio
async def test_local_attachment_change_during_activation_prevents_publication(
    tmp_path: Path,
) -> None:
    authority = SenderAuthority(path=tmp_path / "senders.sqlite3")
    authority.attach_active(sender_id=SENDER, owner_id="same", sender=Candidate())
    entered, resume = asyncio.Event(), asyncio.Event()

    async def pause() -> None:
        entered.set()
        await resume.wait()

    candidate, replacement = Candidate(activate=pause), Candidate()
    handle = authority.stage_activation(sender_id=SENDER, to_owner_id="same", sender=candidate)
    pending = asyncio.create_task(authority.commit_activation(handle))
    try:
        await asyncio.wait_for(entered.wait(), timeout=2)
        authority.attach_active(sender_id=SENDER, owner_id="same", sender=replacement)
        resume.set()
        with pytest.raises(ValueError, match="attachment changed"):
            await asyncio.wait_for(pending, timeout=2)
        with pytest.raises(ValueError, match="attachment changed"):
            await authority.close_activation(handle)
        await dispatch(authority)
        assert candidate.lifecycle == ["activate"] and candidate.calls == []
        assert replacement.lifecycle == [] and len(replacement.calls) == 1
    finally:
        if not pending.done():
            pending.cancel()
            await asyncio.gather(pending, return_exceptions=True)


@pytest.mark.asyncio
async def test_same_owner_sqlite_abort_does_not_publish_candidate(tmp_path: Path) -> None:
    authority = SenderAuthority(path=tmp_path / "senders.sqlite3")
    old, candidate = Candidate(), Candidate()
    authority.attach_active(sender_id=SENDER, owner_id="same", sender=old)
    original = rows(authority.path)
    with closing(sqlite3.connect(authority.path)) as connection, connection:
        connection.execute(
            "CREATE TRIGGER refuse_selection AFTER UPDATE ON sender_authority "
            "BEGIN SELECT RAISE(ABORT, 'selection refused'); END"
        )
    handle = authority.stage_activation(sender_id=SENDER, to_owner_id="same", sender=candidate)
    with pytest.raises(sqlite3.IntegrityError, match="selection refused"):
        await authority.commit_activation(handle)
    await dispatch(authority)
    assert rows(authority.path) == original
    assert len(old.calls) == 1 and candidate.calls == []
    assert candidate.lifecycle == ["activate"]


@pytest.mark.asyncio
@pytest.mark.parametrize("target", ["same", "new"])
async def test_fsync_failure_preserves_selection_uncertainty_and_exact_retry(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, target: str
) -> None:
    authority = SenderAuthority(path=tmp_path / "senders.sqlite3")
    old, candidate = Candidate(), Candidate()
    authority.attach_active(sender_id=SENDER, owner_id="same", sender=old)
    handle = authority.stage_activation(sender_id=SENDER, to_owner_id=target, sender=candidate)

    def fail_fsync(path: Path) -> None:
        raise OSError("durability observation failed")

    with monkeypatch.context() as patch:
        patch.setattr(sender_module, "_fsync_file", fail_fsync)
        with pytest.raises(OSError, match="durability observation failed"):
            await authority.commit_activation(handle)
    selected = rows(authority.path)
    assert authority.active_owner(SENDER) == target
    assert candidate.lifecycle == ["activate"] and candidate.calls == []
    if target == "same":
        await dispatch(authority)
        assert len(old.calls) == 1
    else:
        assert not authority.is_attached(SENDER)
        with pytest.raises(ValueError, match="not attached"):
            await dispatch(authority)
    await authority.commit_activation(handle)
    assert rows(authority.path) == selected
    await dispatch(authority)
    assert candidate.lifecycle == ["activate", "activate"] and len(candidate.calls) == 1


@pytest.mark.asyncio
async def test_returning_to_historical_owner_does_not_revive_old_attachment_on_fsync_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    authority = SenderAuthority(path=tmp_path / "senders.sqlite3")
    original = Candidate()
    authority.attach_active(sender_id=SENDER, owner_id="first", sender=original)
    second = authority.stage_activation(sender_id=SENDER, to_owner_id="second", sender=Candidate())
    await authority.commit_activation(second)
    current = Candidate()
    back = authority.stage_activation(sender_id=SENDER, to_owner_id="first", sender=current)

    def fail_fsync(path: Path) -> None:
        raise OSError("durability observation failed")

    monkeypatch.setattr(sender_module, "_fsync_file", fail_fsync)
    with pytest.raises(OSError):
        await authority.commit_activation(back)
    assert authority.active_owner(SENDER) == "first"
    assert not authority.is_attached(SENDER)
    with pytest.raises(ValueError, match="not attached"):
        await dispatch(authority)
    assert original.calls == current.calls == []


def test_abandoned_staging_does_not_retain_candidate_implementations(tmp_path: Path) -> None:
    authority = SenderAuthority(path=tmp_path / "senders.sqlite3")
    references = []
    for _ in range(25):
        candidate = Candidate()
        references.append(weakref.ref(candidate))
        authority.stage_activation(sender_id=SENDER, to_owner_id="same", sender=candidate)
    del candidate
    gc.collect()
    assert all(reference() is None for reference in references)
    assert authority.active_owner(SENDER) is None


@pytest.mark.asyncio
async def test_superseded_handle_can_release_candidate_without_predecessor_chain(
    tmp_path: Path,
) -> None:
    authority = SenderAuthority(path=tmp_path / "senders.sqlite3")
    first = Candidate()
    first_reference = weakref.ref(first)
    first_handle = authority.stage_activation(sender_id=SENDER, to_owner_id="same", sender=first)
    await authority.commit_activation(first_handle)
    second = Candidate()
    second_handle = authority.stage_activation(sender_id=SENDER, to_owner_id="same", sender=second)
    await authority.commit_activation(second_handle)
    del first
    gc.collect()
    # A caller retaining its exact handle still retains its own candidate.
    assert first_reference() is not None
    del first_handle
    gc.collect()
    assert first_reference() is None
    await authority.commit_activation(second_handle)
    await dispatch(authority)
    assert second.lifecycle == ["activate", "activate"] and len(second.calls) == 1


@pytest.mark.asyncio
async def test_current_handle_retains_replay_and_dropping_it_keeps_active_sender(
    tmp_path: Path,
) -> None:
    authority = SenderAuthority(path=tmp_path / "senders.sqlite3")
    candidate = Candidate()
    reference = weakref.ref(candidate)
    handle = authority.stage_activation(sender_id=SENDER, to_owner_id="same", sender=candidate)
    await authority.commit_activation(handle)
    del candidate
    gc.collect()
    await authority.commit_activation(handle)
    assert reference().lifecycle == ["activate", "activate"]
    del handle
    gc.collect()
    assert reference() is not None and authority.is_attached(SENDER)
    await dispatch(authority)
    assert len(reference().calls) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("completion", ["success", "failure", "cancelled"])
async def test_close_fences_dispatch_before_deactivation_and_preserves_selection(
    tmp_path: Path, completion: str
) -> None:
    authority = SenderAuthority(path=tmp_path / "senders.sqlite3")
    entered, release = asyncio.Event(), asyncio.Event()

    async def deactivate() -> None:
        entered.set()
        await release.wait()
        if completion == "failure":
            raise RuntimeError("provider cleanup uncertain")

    candidate = Candidate(deactivate=deactivate)
    handle = authority.stage_activation(sender_id=SENDER, to_owner_id="same", sender=candidate)
    await authority.commit_activation(handle)
    selected = rows(authority.path)
    borrowed = await authority.select_dispatch(sender_id=SENDER)
    closing = asyncio.create_task(authority.close_activation(handle))
    try:
        await asyncio.wait_for(entered.wait(), timeout=2)
        assert not authority.is_attached(SENDER) and not authority.has_sender(SENDER)
        assert not authority.is_current_dispatch(borrowed)
        with pytest.raises(ValueError, match="not attached"):
            await authority.send(sender_id=SENDER, recipient="recipient", parts=("later",))
        if completion == "cancelled":
            closing.cancel()
            with pytest.raises(asyncio.CancelledError):
                await closing
        else:
            release.set()
            if completion == "failure":
                with pytest.raises(RuntimeError, match="cleanup uncertain"):
                    await closing
            else:
                await closing
        assert rows(authority.path) == selected
        assert not authority.is_attached(SENDER)
        with pytest.raises(ValueError, match="not attached"):
            await dispatch(authority)
        assert candidate.calls == []
        # Only a new explicit activation may restore dispatch, including after
        # a failed or cancelled provider cleanup. Its external effects are unknown.
        await authority.commit_activation(handle)
        assert rows(authority.path) == selected
        assert not authority.is_current_dispatch(borrowed)
        assert (await dispatch(authority))["provider_message_id"] == "fixture-result"
        assert candidate.lifecycle == ["activate", "deactivate", "activate"]
    finally:
        if not closing.done():
            closing.cancel()
            await asyncio.gather(closing, return_exceptions=True)


@pytest.mark.asyncio
async def test_close_does_not_block_or_invalidate_an_independent_sender(tmp_path: Path) -> None:
    authority = SenderAuthority(path=tmp_path / "senders.sqlite3")
    entered, release = asyncio.Event(), asyncio.Event()

    async def deactivate() -> None:
        entered.set()
        await release.wait()
        raise RuntimeError("provider cleanup uncertain")

    closing_sender, independent = Candidate(deactivate=deactivate), Candidate()
    handle = authority.stage_activation(
        sender_id=SENDER, to_owner_id="same", sender=closing_sender
    )
    await authority.commit_activation(handle)
    other = authority.stage_activation(
        sender_id="independent", to_owner_id="other", sender=independent
    )
    await authority.commit_activation(other)
    selected = rows(authority.path)
    borrowed = await authority.select_dispatch(sender_id="independent")
    closing = asyncio.create_task(authority.close_activation(handle))
    try:
        await asyncio.wait_for(entered.wait(), timeout=2)
        assert authority.is_current_dispatch(borrowed)
        result = await asyncio.wait_for(
            authority.dispatch(
                sender_id="independent", recipient="recipient", part_index=0,
                part={"text": "independent"},
            ), timeout=2,
        )
        assert result["outcome"] == "sent"
        # The independent lifecycle can also proceed while the first is closing.
        await asyncio.wait_for(authority.commit_activation(other), timeout=2)
        release.set()
        with pytest.raises(RuntimeError, match="cleanup uncertain"):
            await closing
        assert rows(authority.path) == selected
        assert authority.is_attached("independent") and not authority.is_attached(SENDER)
        assert len(independent.calls) == 1 and closing_sender.calls == []
        assert independent.lifecycle == ["activate", "activate"]
    finally:
        if not closing.done():
            closing.cancel()
            await asyncio.gather(closing, return_exceptions=True)


@pytest.mark.asyncio
async def test_close_serializes_exact_handle_reactivation(tmp_path: Path) -> None:
    authority = SenderAuthority(path=tmp_path / "senders.sqlite3")
    entered, release = asyncio.Event(), asyncio.Event()

    async def deactivate() -> None:
        entered.set()
        await release.wait()

    candidate = Candidate(deactivate=deactivate)
    handle = authority.stage_activation(sender_id=SENDER, to_owner_id="same", sender=candidate)
    await authority.commit_activation(handle)
    selected = rows(authority.path)
    closing = asyncio.create_task(authority.close_activation(handle))
    replay = None
    try:
        await asyncio.wait_for(entered.wait(), timeout=2)
        replay = asyncio.create_task(authority.commit_activation(handle))
        await asyncio.sleep(0)
        assert not replay.done()
        assert candidate.lifecycle == ["activate", "deactivate"]
        assert not authority.is_attached(SENDER)
        release.set()
        await asyncio.wait_for(asyncio.gather(closing, replay), timeout=2)
        assert candidate.lifecycle == ["activate", "deactivate", "activate"]
        assert rows(authority.path) == selected
        await dispatch(authority)
        assert len(candidate.calls) == 1
    finally:
        pending = [task for task in (closing, replay) if task is not None and not task.done()]
        for task in pending:
            task.cancel()
        await asyncio.gather(*pending, return_exceptions=True)


@pytest.mark.asyncio
async def test_close_replacement_survives_old_deactivation_and_same_object_is_refused(
    tmp_path: Path,
) -> None:
    authority = SenderAuthority(path=tmp_path / "senders.sqlite3")
    entered, release = asyncio.Event(), asyncio.Event()

    async def deactivate() -> None:
        entered.set()
        await release.wait()

    candidate, replacement = Candidate(deactivate=deactivate), Candidate()
    handle = authority.stage_activation(sender_id=SENDER, to_owner_id="same", sender=candidate)
    await authority.commit_activation(handle)
    closing = asyncio.create_task(authority.close_activation(handle))
    try:
        await asyncio.wait_for(entered.wait(), timeout=2)
        selected = rows(authority.path)
        with pytest.raises(ValueError, match="deactivating"):
            authority.attach_active(sender_id=SENDER, owner_id="same", sender=candidate)
        assert rows(authority.path) == selected and not authority.is_attached(SENDER)
        authority.attach_active(sender_id=SENDER, owner_id="same", sender=replacement)
        replacement_rows = rows(authority.path)
        release.set()
        await asyncio.wait_for(closing, timeout=2)
        assert rows(authority.path) == replacement_rows and authority.is_attached(SENDER)
        for operation in (authority.commit_activation, authority.close_activation):
            with pytest.raises(ValueError, match="attachment changed"):
                await operation(handle)
        await dispatch(authority)
        assert candidate.calls == [] and len(replacement.calls) == 1
        assert candidate.lifecycle == ["activate", "deactivate"] and replacement.lifecycle == []
    finally:
        if not closing.done():
            closing.cancel()
            await asyncio.gather(closing, return_exceptions=True)


@pytest.mark.asyncio
async def test_closed_handle_release_does_not_keep_deactivated_sender_attached(tmp_path: Path) -> None:
    authority = SenderAuthority(path=tmp_path / "senders.sqlite3")
    candidate = Candidate()
    reference = weakref.ref(candidate)
    handle = authority.stage_activation(sender_id=SENDER, to_owner_id="same", sender=candidate)
    await authority.commit_activation(handle)
    selected = rows(authority.path)
    await authority.close_activation(handle)
    del candidate, handle
    gc.collect()
    assert reference() is None and not authority.is_attached(SENDER)
    assert rows(authority.path) == selected


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "target,reuse,attached",
    [("same", False, True), ("same", True, False), ("new", False, False)],
)
async def test_close_after_fsync_failure_fences_only_the_deactivated_implementation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, target: str, reuse: bool, attached: bool
) -> None:
    authority = SenderAuthority(path=tmp_path / "senders.sqlite3")
    old = Candidate()
    authority.attach_active(sender_id=SENDER, owner_id="same", sender=old)
    candidate = old if reuse else Candidate()
    handle = authority.stage_activation(sender_id=SENDER, to_owner_id=target, sender=candidate)

    def fail_fsync(path: Path) -> None:
        raise OSError("durability observation failed")

    with monkeypatch.context() as patch:
        patch.setattr(sender_module, "_fsync_file", fail_fsync)
        with pytest.raises(OSError, match="durability observation failed"):
            await authority.commit_activation(handle)
    selected = rows(authority.path)
    await authority.close_activation(handle)
    assert rows(authority.path) == selected
    assert authority.is_attached(SENDER) is attached
    assert candidate.lifecycle == ["activate", "deactivate"]
    if attached:
        await dispatch(authority)
        assert len(old.calls) == 1 and candidate.calls == []
    else:
        with pytest.raises(ValueError, match="not attached"):
            await dispatch(authority)
    await authority.commit_activation(handle)
    assert rows(authority.path) == selected
    await dispatch(authority)
    assert len(candidate.calls) == 1


@pytest.mark.asyncio
async def test_losing_activation_cleanup_cannot_deactivate_a_same_object_reattachment(
    tmp_path: Path,
) -> None:
    path = tmp_path / "senders.sqlite3"
    authority, winner = SenderAuthority(path=path), SenderAuthority(path=path)
    activating, finish_activation = asyncio.Event(), asyncio.Event()
    deactivating, finish_deactivation = asyncio.Event(), asyncio.Event()

    async def activate() -> None:
        activating.set()
        await finish_activation.wait()

    async def deactivate() -> None:
        deactivating.set()
        await finish_deactivation.wait()

    candidate = Candidate(activate=activate, deactivate=deactivate)
    authority.attach_active(sender_id=SENDER, owner_id="old", sender=candidate)
    losing = authority.stage_activation(sender_id=SENDER, to_owner_id="loser", sender=candidate)
    winning_sender = Candidate()
    winning = winner.stage_activation(sender_id=SENDER, to_owner_id="winner", sender=winning_sender)
    pending = asyncio.create_task(authority.commit_activation(losing))
    try:
        await asyncio.wait_for(activating.wait(), timeout=2)
        await winner.commit_activation(winning)
        selected = rows(path)
        finish_activation.set()
        await asyncio.wait_for(deactivating.wait(), timeout=2)
        with pytest.raises(ValueError, match="deactivating"):
            authority.attach_active(sender_id=SENDER, owner_id="winner", sender=candidate)
        assert rows(path) == selected and not authority.is_attached(SENDER)
        replacement = Candidate()
        authority.attach_active(sender_id=SENDER, owner_id="winner", sender=replacement)
        replacement_rows = rows(path)
        finish_deactivation.set()
        with pytest.raises(ValueError, match="changed during activation"):
            await asyncio.wait_for(pending, timeout=2)
        assert rows(path) == replacement_rows
        assert authority.active_owner(SENDER) == winner.active_owner(SENDER) == "winner"
        with pytest.raises(ValueError, match="attachment changed"):
            await authority.close_activation(losing)
        await dispatch(authority)
        await dispatch(winner)
        assert len(replacement.calls) == len(winning_sender.calls) == 1
        assert candidate.calls == [] and candidate.lifecycle == ["activate", "deactivate"]
        assert winning_sender.lifecycle == ["activate"] and replacement.lifecycle == []
    finally:
        if not pending.done():
            pending.cancel()
            await asyncio.gather(pending, return_exceptions=True)
