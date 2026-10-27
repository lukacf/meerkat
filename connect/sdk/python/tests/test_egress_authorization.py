"""Current host authorization at the existing durable delivery boundary.

Real SQLite rows and provider-call observations; policy decisions are host
fixtures, not proof of native permissions or live-provider audience checks.
"""

from __future__ import annotations

import asyncio
from dataclasses import FrozenInstanceError
import hashlib
import json
from pathlib import Path
import sqlite3

import pytest

from meerkat_toolkit.frontend import EgressAuthorizationDecision as Decision
from meerkat_toolkit.frontend import EgressDeliveryContext
from meerkat_toolkit.frontend.egress import ProactiveEgress
from meerkat_toolkit.frontend.sender import SenderAuthority


def engine(path: Path, provider, authorize=None, **kwargs):
    return ProactiveEgress(
        path=path,
        senders={"sender": provider},
        incident_sink=lambda _: None,
        health_sink=kwargs.pop("health_sink", lambda _: None),
        sleep=kwargs.pop("sleep", lambda _: None),
        authorization_mode=kwargs.pop("authorization_mode", "enforced"),
        authorize_delivery=authorize,
        **kwargs,
    )


def commit(owner, *, parts=({"text": "hello"},), key="work/one"):
    return owner.commit_output(
        issuer_id="issuer/person",
        logical_key=key,
        candidates=(("sender", "recipient/thread"),),
        parts=parts,
        incident_delivery=False,
    )


def rows(path):
    with sqlite3.connect(path) as db:
        return db.execute(
            "SELECT part_index,state,attempts,unknown_attempts,provider_msg_id,error,last_outcome "
            "FROM egress_parts ORDER BY part_index"
        ).fetchall()


@pytest.mark.parametrize(
    "mode,callback",
    [
        ("other", None),
        ("enforced", None),
        ("enforced", True),
        ("legacy", lambda _: Decision.ALLOWED),
        ("legacy", False),
    ],
)
def test_invalid_authorization_profile_refuses_before_creating_database(tmp_path, mode, callback):
    path = tmp_path / "outbox.sqlite3"
    with pytest.raises(ValueError):
        engine(path, lambda **_: None, callback, authorization_mode=mode)
    assert not path.exists()


async def test_legacy_profile_preserves_existing_unchecked_contract(tmp_path):
    calls = []
    owner = engine(
        tmp_path / "outbox",
        lambda **kw: calls.append(kw) or {"outcome": "sent"},
        authorization_mode="legacy",
    )
    output = commit(owner)
    await owner._dispatch_output(output)
    assert len(calls) == 1
    assert owner.status(output_id=output).delivery_state == "provider_accepted"


async def test_exact_context_is_frozen_and_precedes_attempt_reservation(tmp_path):
    path = tmp_path / "outbox"
    contexts, calls = [], []
    part = {"text": "hello", "entities": [{"type": "bold", "offset": 0, "length": 5}]}

    def authorize(context):
        contexts.append(context)
        assert isinstance(context, EgressDeliveryContext)
        assert rows(path)[0][2:4] == (0, 0)
        with pytest.raises(FrozenInstanceError):
            context.recipient = "different"
        with pytest.raises(TypeError):
            context.part["text"] = "different"
        with pytest.raises(TypeError):
            context.part["entities"][0]["length"] = 1
        return Decision.ALLOWED

    def provider(**kwargs):
        calls.append(kwargs)
        assert rows(path)[0][2:4] == (1, 1)
        return {"outcome": "sent", "provider_message_id": "receipt"}

    owner = engine(path, provider, authorize, normalize_parts=lambda p: p)
    output = commit(owner, parts=(part,))
    part["text"] = "caller mutation"
    part["entities"][0]["length"] = 1
    await owner._dispatch_output(output)
    context = contexts[0]
    assert (
        context.output_id,
        context.issuer_id,
        context.logical_key,
        context.sender_id,
        context.recipient,
        context.part_index,
    ) == (output, "issuer/person", "work/one", "sender", "recipient/thread", 0)
    retained = {"text": "hello", "entities": [{"type": "bold", "offset": 0, "length": 5}]}
    encoded = json.dumps(
        {
            "candidates": [["sender", "recipient/thread"]],
            "parts": [retained],
            "incident_delivery": False,
        },
        separators=(",", ":"),
        sort_keys=True,
    )
    assert context.request_digest == hashlib.sha256(encoded.encode()).hexdigest()
    assert calls[0]["part"]["text"] == "hello"
    assert calls[0]["part"]["entities"][0]["length"] == 5


@pytest.mark.parametrize("decision", [Decision.DENIED, Decision.DEFERRED])
async def test_refusal_preserves_pending_and_reopen_rechecks_current_host_policy(
    tmp_path, decision
):
    path = tmp_path / "outbox"
    calls, contexts, health = [], [], []

    def provider(**kwargs):
        return calls.append(kwargs) or {"outcome": "sent"}

    first = engine(
        path, provider, lambda c: contexts.append(c) or decision, health_sink=health.append
    )
    output = commit(first)
    before = rows(path)
    await first._dispatch_output(output)
    assert rows(path) == before
    assert calls == []
    assert health[-1]["kind"] == "egress_authorization_" + decision.value
    second = engine(path, provider, lambda c: contexts.append(c) or Decision.ALLOWED)
    assert commit(second) == output
    await second.recover_pending()
    assert len(calls) == 1 and len(contexts) == 2
    assert contexts[0] == contexts[1]
    assert rows(path)[0][1:4] == ("sent", 1, 0)


@pytest.mark.parametrize("result", [None, True, "allowed", {"decision": "allowed"}, object()])
async def test_malformed_decision_is_deferred_without_provider_attempt(tmp_path, result):
    path = tmp_path / "outbox"
    calls, health = [], []
    owner = engine(path, lambda **kw: calls.append(kw), lambda _: result, health_sink=health.append)
    output = commit(owner)
    before = rows(path)
    await owner._dispatch_output(output)
    assert rows(path) == before and calls == []
    assert health == [
        {
            "kind": "egress_authorization_deferred",
            "output_id": output,
            "part_index": 0,
            "code": "invalid_authorization_decision",
        }
    ]


async def test_callback_failure_uses_fixed_safe_diagnostic(tmp_path):
    path = tmp_path / "outbox"
    calls, health = [], []

    def authorize(_):
        raise RuntimeError("PRIVATE POLICY DETAIL")

    owner = engine(path, lambda **kw: calls.append(kw), authorize, health_sink=health.append)
    output = commit(owner)
    before = rows(path)
    await owner._dispatch_output(output)
    assert rows(path) == before and calls == []
    assert health[-1]["code"] == "authorization_unavailable"
    assert "PRIVATE" not in repr(health) + repr(rows(path))


@pytest.mark.parametrize(
    "field,value",
    [("issuer_id", ""), ("logical_key", " "), ("request_digest", ""), ("request_digest", "g" * 64)],
)
async def test_missing_retained_identity_never_reaches_authorizer_or_provider(
    tmp_path, field, value
):
    path = tmp_path / "outbox"
    calls, authorized, health = [], [], []
    owner = engine(
        path,
        lambda **kw: calls.append(kw),
        lambda c: authorized.append(c) or Decision.ALLOWED,
        health_sink=health.append,
    )
    output = commit(owner)
    with sqlite3.connect(path) as db:
        db.execute(f"UPDATE egress_outputs SET {field}=?", (value,))
    before = rows(path)
    await owner._dispatch_output(output)
    assert rows(path) == before and calls == [] and authorized == []
    assert health[-1]["code"] == "invalid_delivery_context"


async def test_legitimate_legacy_backfill_keeps_identity_for_enforced_adoption(tmp_path):
    path = tmp_path / "outbox"
    legacy = engine(path, lambda **_: None, authorization_mode="legacy")
    output = commit(legacy)
    with sqlite3.connect(path) as db:
        db.execute("ALTER TABLE egress_outputs DROP COLUMN root")
        db.execute("ALTER TABLE egress_outputs DROP COLUMN part_count")
    contexts, calls = [], []
    owner = engine(
        path,
        lambda **kw: calls.append(kw) or {"outcome": "sent"},
        lambda c: contexts.append(c) or Decision.ALLOWED,
    )
    await owner.recover_pending()
    assert len(calls) == 1
    assert contexts[0].output_id == output
    assert contexts[0].issuer_id == "issuer/person" and contexts[0].logical_key == "work/one"
    with sqlite3.connect(path) as db:
        assert db.execute("SELECT root,part_count FROM egress_outputs").fetchone() == (
            "hc-notify-v1:issuer/person/work/one",
            1,
        )


async def test_retry_refusal_does_not_spend_remaining_provider_budget(tmp_path):
    path = tmp_path / "outbox"
    calls, contexts = [], []
    allowed = True

    def authorize(context):
        contexts.append(context)
        return Decision.ALLOWED if allowed else Decision.DENIED

    def provider(**kwargs):
        nonlocal allowed
        calls.append(kwargs)
        allowed = False
        return {"outcome": "known_not_sent", "error": "busy"}

    owner = engine(path, provider, authorize)
    output = commit(owner)
    await owner._dispatch_output(output)
    assert len(calls) == 1 and len(contexts) == 2
    assert rows(path)[0] == (0, "pending", 1, 0, None, "busy", "known_not_sent")
    reopened = engine(
        path,
        lambda **kw: calls.append(kw) or {"outcome": "sent"},
        lambda c: contexts.append(c) or Decision.ALLOWED,
    )
    await reopened.recover_pending()
    assert len(calls) == 2 and len(contexts) == 3
    assert all(context == contexts[0] for context in contexts)
    assert rows(path)[0][1:4] == ("sent", 2, 0)


@pytest.mark.parametrize("first_outcome", ["sent", "unknown"])
async def test_multipart_reopen_preserves_accepted_or_unknown_part(tmp_path, first_outcome):
    path = tmp_path / "outbox"
    calls, authorized = [], []
    owner = engine(
        path,
        lambda **kw: calls.append(kw) or {"outcome": first_outcome},
        lambda c: authorized.append(c.part_index)
        or (Decision.ALLOWED if c.part_index == 0 else Decision.DEFERRED),
    )
    output = commit(owner, parts=({"text": "one"}, {"text": "two"}))
    await owner._dispatch_output(output)
    first = rows(path)[0]
    assert len(calls) == 1 and authorized == [0, 1]
    reopened = engine(
        path,
        lambda **kw: calls.append(kw) or {"outcome": "sent"},
        lambda c: authorized.append(c.part_index) or Decision.ALLOWED,
    )
    await reopened.recover_pending()
    assert rows(path)[0] == first
    assert [call["part_index"] for call in calls] == [0, 1]
    assert authorized == [0, 1, 1]


async def test_policy_timeout_and_cancellation_do_not_reserve_attempt(tmp_path):
    for cancel in (False, True):
        path = tmp_path / str(cancel)
        entered = asyncio.Event()
        calls = []

        async def authorize(_):
            entered.set()
            await asyncio.Event().wait()

        owner = engine(
            path, lambda **kw: calls.append(kw), authorize, dispatch_timeout_seconds=0.03
        )
        output = commit(owner)
        before = rows(path)
        task = asyncio.create_task(owner._dispatch_output(output))
        await entered.wait()
        if cancel:
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await task
        else:
            await task
        assert rows(path) == before and calls == []


async def test_sender_authority_path_also_requires_current_authorization(tmp_path):
    calls = []
    authority = SenderAuthority(path=tmp_path / "senders")
    authority.attach_active(
        sender_id="sender",
        owner_id="active",
        sender=lambda **kw: calls.append(kw) or {"outcome": "sent"},
    )
    owner = engine(
        tmp_path / "outbox",
        lambda **_: pytest.fail("legacy sender used"),
        lambda _: Decision.DENIED,
        sender_authority=authority,
    )
    output = commit(owner)
    await owner._dispatch_output(output)
    assert calls == [] and rows(owner.path)[0][1:4] == ("pending", 0, 0)


@pytest.mark.parametrize("transition", ["same_attachment", "replace", "close", "failed_activation"])
async def test_sender_lifecycle_during_policy_invalidates_borrow_before_reservation(
    tmp_path, transition
):
    calls = []
    entered, release = asyncio.Event(), asyncio.Event()
    authority = SenderAuthority(path=tmp_path / "senders")

    class Sender:
        fail_activation = False

        async def activate(self):
            if self.fail_activation:
                raise RuntimeError("activation did not complete")

        async def deactivate(self):
            pass

        async def __call__(self, **kwargs):
            calls.append(kwargs)
            return {"outcome": "sent"}

    original = Sender()
    activation = authority.stage_activation(sender_id="sender", to_owner_id="old", sender=original)
    await authority.commit_activation(activation)

    async def authorize(_):
        entered.set()
        await release.wait()
        return Decision.ALLOWED

    owner = engine(tmp_path / "outbox", original, authorize, sender_authority=authority)
    output = commit(owner)
    task = asyncio.create_task(owner._dispatch_output(output))
    await entered.wait()
    if transition == "same_attachment":
        authority.attach_active(sender_id="sender", owner_id="old", sender=original)
    elif transition == "replace":
        replacement = authority.stage_activation(
            sender_id="sender", to_owner_id="new", sender=Sender()
        )
        await authority.commit_activation(replacement)
    elif transition == "close":
        await authority.close_activation(activation)
    else:
        original.fail_activation = True
        with pytest.raises(RuntimeError):
            await authority.commit_activation(activation)
    release.set()
    await task
    assert calls == []
    assert rows(owner.path)[0][1:4] == ("pending", 0, 0)


@pytest.mark.parametrize("mode", ["legacy", "enforced"])
async def test_closed_sender_retains_pending_work_until_explicit_reactivation(tmp_path, mode):
    calls = []

    class Sender:
        async def activate(self):
            pass

        async def deactivate(self):
            raise RuntimeError("provider cleanup uncertain")

        async def __call__(self, **kwargs):
            calls.append(kwargs)
            return {"outcome": "sent", "provider_message_id": "recovered-receipt"}

    authority = SenderAuthority(path=tmp_path / "senders")
    sender = Sender()
    handle = authority.stage_activation(sender_id="sender", to_owner_id="active", sender=sender)
    await authority.commit_activation(handle)
    owner = engine(
        tmp_path / "outbox", lambda **_: pytest.fail("fallback sender used"),
        (lambda _: Decision.ALLOWED) if mode == "enforced" else None,
        authorization_mode=mode, sender_authority=authority,
    )
    output = commit(owner)
    before = rows(owner.path)
    with pytest.raises(RuntimeError, match="cleanup uncertain"):
        await authority.close_activation(handle)
    summary = await owner.run_recovery_pass(respect_grace=False)
    assert summary["skipped_detached"] == 1 and summary["dispatched"] == 0
    assert calls == [] and rows(owner.path) == before
    assert owner.status(output_id=output).part_states == ("pending",)
    assert not owner.status(output_id=output).cancel_requested
    await authority.commit_activation(handle)
    summary = await owner.run_recovery_pass(respect_grace=False)
    assert summary["dispatched"] == 1
    assert len(calls) == 1 and calls[0]["part_index"] == 0
    assert rows(owner.path)[0][1:5] == ("sent", 1, 0, "recovered-receipt")
    assert owner.status(output_id=output).delivery_state == "provider_accepted"


@pytest.mark.parametrize("mode", ["legacy", "enforced"])
@pytest.mark.parametrize("outcome,unknown", [("sent", 0), ("unknown", 1)])
async def test_close_preserves_entered_receipt_and_leaves_later_parts_pending(
    tmp_path, mode, outcome, unknown
):
    entered, release = asyncio.Event(), asyncio.Event()
    calls = []

    class Sender:
        async def activate(self):
            pass

        async def deactivate(self):
            pass

        async def __call__(self, **kwargs):
            calls.append(kwargs)
            entered.set()
            await release.wait()
            return {"outcome": outcome, "provider_message_id": "entered-receipt"}

    authority = SenderAuthority(path=tmp_path / "senders")
    sender = Sender()
    handle = authority.stage_activation(sender_id="sender", to_owner_id="active", sender=sender)
    await authority.commit_activation(handle)
    owner = engine(
        tmp_path / "outbox", lambda **_: pytest.fail("fallback sender used"),
        (lambda _: Decision.ALLOWED) if mode == "enforced" else None,
        authorization_mode=mode, sender_authority=authority, normalize_parts=lambda parts: parts,
    )
    output = commit(owner, parts=({"text": "entered"}, {"text": "later"}))
    pending = owner.spawn_dispatch(output)
    try:
        await asyncio.wait_for(entered.wait(), timeout=2)
        await authority.close_activation(handle)
        assert not pending.done()
        release.set()
        await asyncio.wait_for(pending, timeout=2)
        assert [call["part_index"] for call in calls] == [0]
        stored = rows(owner.path)
        assert stored[0][2:5] == (1, unknown, "entered-receipt")
        assert stored[0][-1] == ("provider_accepted" if outcome == "sent" else "unknown")
        assert stored[1][1:5] == ("pending", 0, 0, None)
        assert not owner.status(output_id=output).cancel_requested
        summary = await owner.run_recovery_pass(respect_grace=False)
        assert summary["skipped_detached"] == 1 and summary["dispatched"] == 0
        assert rows(owner.path) == stored
        assert len(calls) == 1
    finally:
        if not pending.done():
            pending.cancel()
            await asyncio.gather(pending, return_exceptions=True)


async def test_missing_part_payload_is_deferred_as_invalid_context(tmp_path):
    path = tmp_path / "outbox"
    calls, contexts, health = [], [], []
    owner = engine(
        path,
        lambda **kw: calls.append(kw),
        lambda c: contexts.append(c) or Decision.ALLOWED,
        health_sink=health.append,
    )
    output = commit(owner)
    with sqlite3.connect(path) as db:
        db.execute("UPDATE egress_parts SET text=NULL")
    await owner._dispatch_output(output)
    assert calls == [] and contexts == []
    assert rows(path)[0][1:4] == ("pending", 0, 0)
    assert health[-1]["code"] == "invalid_delivery_context"
