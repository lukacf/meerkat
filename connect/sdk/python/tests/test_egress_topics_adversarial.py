"""Independent thread-target checks at the durable output boundary.

Policies below are host fixtures, not native admission or live audience proof.
"""

from __future__ import annotations

import hashlib
import json
import sqlite3

import pytest

from meerkat_toolkit.frontend.egress import (
    EgressAuthorizationDecision as Decision,
    EgressCandidate,
    EgressMigrationBinding,
    ProactiveEgress,
)
from meerkat_toolkit.frontend.sender import SenderAuthority


def managed(path, sender, policy, **kwargs):
    if not path.exists():
        ProactiveEgress.migrate_format(
            path=path, expected=ProactiveEgress.inspect_format(path),
            binding=EgressMigrationBinding("1" * 64, "2" * 64, "3" * 64),
        )
    return ProactiveEgress(
        path=path, schema_mode="managed", senders={"telegram": sender},
        incident_sink=lambda _: None, health_sink=lambda _: None,
        sleep=lambda _: None, authorization_mode="enforced",
        authorize_delivery=policy, **kwargs,
    )


def commit(owner, *, issuer="alice", key="summary", thread="42", text="private"):
    return owner.commit_output(
        issuer_id=issuer, logical_key=key,
        candidates=[EgressCandidate("telegram", "-10077", thread)],
        parts=[{"text": text}], incident_delivery=False,
    )


def retained(path, output):
    with sqlite3.connect(path) as connection:
        return connection.execute(
            "SELECT request_digest,thread_id FROM egress_outputs WHERE output_id=?", (output,)
        ).fetchone()


async def test_same_chat_different_issuer_and_thread_require_distinct_policy(tmp_path):
    calls, contexts = [], []

    async def sender(*, thread_id, **kwargs):
        calls.append((thread_id, kwargs["part"]["text"]))
        return {"outcome": "sent", "provider_message_id": "7"}

    def policy(context):
        contexts.append((context.issuer_id, context.thread_id))
        return Decision.ALLOWED if (context.issuer_id, context.thread_id) == ("alice", "42") else Decision.DENIED

    owner = managed(tmp_path / "outbox.db", sender, policy)
    permitted = commit(owner)
    other_thread = commit(owner, key="other-thread", thread="43")
    other_issuer = commit(owner, issuer="bob")
    for output in (permitted, other_thread, other_issuer):
        await owner._dispatch_output(output)
    assert calls == [("42", "private")]
    assert contexts == [("alice", "42"), ("alice", "43"), ("bob", "42")]
    assert owner.attempt_count(output_id=other_thread) == 0
    assert owner.attempt_count(output_id=other_issuer) == 0


async def test_sender_replaced_during_authorization_cannot_drop_thread(tmp_path):
    calls = []
    authority = SenderAuthority(path=tmp_path / "senders.db")

    async def supported(*, thread_id, **kwargs):
        calls.append(thread_id)
        return {"outcome": "sent", "provider_message_id": "7"}

    async def ignores_unknown_keywords(**kwargs):
        calls.append("wrong-parent-chat")
        return {"outcome": "sent"}

    authority.attach_active(sender_id="telegram", owner_id="host", sender=supported)
    replace = [True]

    def policy(context):
        assert context.thread_id == "42"
        if replace[0]:
            replace[0] = False
            authority.attach_active(
                sender_id="telegram", owner_id="host", sender=ignores_unknown_keywords,
            )
        return Decision.ALLOWED

    owner = managed(tmp_path / "outbox.db", supported, policy, sender_authority=authority)
    output = commit(owner)
    await owner._dispatch_output(output)
    await owner._dispatch_output(output)
    assert calls == [] and owner.attempt_count(output_id=output) == 0
    authority.attach_active(sender_id="telegram", owner_id="host", sender=supported)
    await owner._dispatch_output(output)
    assert calls == ["42"] and owner.attempt_count(output_id=output) == 1


@pytest.mark.parametrize("bad_thread", ["", " ", "42\n", "x" * 1025, b"42"])
async def test_invalid_retained_thread_never_reaches_permissive_host_or_sender(tmp_path, bad_thread):
    calls = []

    async def sender(*, thread_id, **kwargs):
        calls.append(thread_id)
        return {"outcome": "sent"}

    owner = managed(tmp_path / "outbox.db", sender, lambda _: Decision.ALLOWED)
    output = commit(owner)
    with sqlite3.connect(owner.path) as connection:
        connection.execute("UPDATE egress_outputs SET thread_id=? WHERE output_id=?", (bad_thread, output))
    try:
        await owner._dispatch_output(output)
    except (ValueError, TypeError):
        pass
    assert calls == [] and owner.attempt_count(output_id=output) == 0


def test_changing_only_thread_conflicts_and_leaves_original_request(tmp_path):
    owner = managed(tmp_path / "outbox.db", None, lambda _: Decision.DENIED)
    output = commit(owner)
    before = retained(owner.path, output)
    with pytest.raises(ValueError):
        commit(owner, thread="43")
    with pytest.raises(ValueError):
        commit(owner, thread=None)
    assert retained(owner.path, output) == before
    assert commit(owner) == output


async def test_no_thread_keeps_legacy_digest_and_keyword_shape(tmp_path):
    calls = []

    async def sender(**kwargs):
        calls.append(kwargs)
        return {"outcome": "sent", "provider_message_id": "8"}

    owner = managed(tmp_path / "outbox.db", sender, lambda _: Decision.ALLOWED)
    output = commit(owner, thread=None)
    expected = hashlib.sha256(json.dumps({
        "candidates": [["telegram", "-10077"]], "parts": [{"text": "private"}],
        "incident_delivery": False,
    }, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()).hexdigest()
    assert retained(owner.path, output) == (expected, None)
    await owner._dispatch_output(output)
    assert len(calls) == 1 and set(calls[0]) == {"sender_id", "recipient", "part_index", "part"}


@pytest.mark.parametrize("damage", ["remove", "older_version"])
async def test_open_owner_rechecks_format_before_recovered_thread_entry(tmp_path, damage):
    calls = []

    async def sender(*, thread_id, **kwargs):
        calls.append(thread_id)
        return {"outcome": "sent"}

    owner = managed(tmp_path / "outbox.db", sender, lambda _: Decision.ALLOWED)
    output = commit(owner)
    with sqlite3.connect(owner.path) as connection:
        if damage == "remove":
            connection.execute("DROP TABLE egress_format")
        else:
            connection.execute("UPDATE egress_format SET format='egress-sqlite-2'")
    with pytest.raises(ValueError):
        await owner._dispatch_output(output)
    assert calls == [] and owner.attempt_count(output_id=output) == 0
