"""Mob announcement sources on the Python SDK: wire names, typed results,
open vocabularies, typed errors and the capability seam.

The transport is mocked. Each row checks the exact method name and params the
SDK sends and how it reads the gateway's answer, so a wire change on either
side fails here first.
"""

import pytest

from meerkat_meta import ErrorEvent
from meerkat_meta.errors import RpcError

from .test_rpc_method_names import make_mock_mob_handle

MEMBER = {"mob_id": "mob-a", "identity": "lead:main"}

ANNOUNCEMENT = {
    "mob_id": "mob-a",
    "sequence": 7,
    "announced_by": {"kind": "host"},
    "source_key": "ops/1",
    "topics": ["ops.alert"],
    "body": "Disk full on db-2",
    "correlation_id": "c-1",
    "causes": [{"mob_id": "mob-a", "source_key": "ops/0"}],
    "digest": "sha256:ab",
    "announced_at_ms": 1000,
    "a_future_field": True,
}

SUBSCRIPTION = {
    "id": "sub-1",
    "member": MEMBER,
    "selector": {"topics": ["ops.alert"], "mobs": ["mob-a"]},
    "state": "active",
    "revision": 2,
    "selector_revision": 1,
    "frontier": {"mob-a": 6},
    "created_at_ms": 10,
    "updated_at_ms": 20,
}

BATCH = {
    "id": "batch-1",
    "member": MEMBER,
    "items": [{"subscription_id": "sub-1", "subscription_revision": 2, "mob_id": "mob-a",
               "sequence": 7, "source_key": "ops/1"}],
    "gaps": [{"mob_id": "mob-a", "count": 2, "reason": "expired"}],
    "route": "local",
    "body": "1 announcement",
    "content_digest": "sha256:cd",
    "state": {"state": "delivered", "receipt": {"delivery_id": "d-1"}},
    "sealed_at_ms": 30,
    "updated_at_ms": 40,
    "handling": {"state": "handled"},
}

SOURCE = {
    "mob_id": "mob-a",
    "source_id": "mail",
    "generation": 1,
    "revision": 3,
    "identity": {
        "provider": "mail",
        "principal": {"kind": "connector_verified_account", "issuer": "https://issuer.example",
                      "strategy_id": "oauth", "subject": "user-1"},
        "scope": {"kind": "inbox", "id": "INBOX"},
    },
    "connection": {"kind": "connect_setup", "id": "conn-1"},
    "observation": {"baseline": "snapshot", "deletions": "explicit"},
    "topics": ["mail.new"],
    "audience": [{"kind": "role", "mob_id": "mob-a", "role": "lead"}],
    "routes": [{"kind": "direct"}, {"kind": "a_future_route", "lane": 2}],
    "conversation_members": [],
    "start": {"kind": "baseline"},
    "delivery": {"priority": "normal", "max_per_batch": 20, "on_unhandled": "redeliver"},
    "health_policy": {"escalate_after_failures": 3, "escalate_after_ms": 3600000,
                      "stale_after_ms": 7200000, "quiet_after_ms": None, "empty_after_ms": None},
    "runner": {"host_runnable": "mail_poll", "every_seconds": 300},
    "state": "active",
    "collection": "live",
    "cursor": {"version": 4, "token": "opaque-token", "advanced_at_ms": 50},
    "streams": {"live": 1, "onboarding": 0},
    "backfilled": 0,
    "health": {
        "status": "ok", "last_success_at_ms": 50, "last_new_item_at_ms": 40,
        "last_observed": {"kind": "delta"}, "last_observed_at_ms": 50,
        "empty_snapshot_since_ms": None, "consecutive_failures": 0,
        "totals": {"announced": 5, "replayed": 1, "conflicts": 0, "own_writes": 0,
                   "refusals": 0, "gaps": 0},
        "last_failure": None, "last_conflict": None,
    },
    "created_at_ms": 1,
    "updated_at_ms": 50,
}

ADVANCE = {
    "source": SOURCE,
    "replayed": False,
    "items": [
        {"item_id": "m1", "version": "v1", "source_key": "src:mail/m1", "outcome": "announced",
         "sequence": 8, "captured": 2},
        {"item_id": "m2", "version": "v2", "source_key": "src:mail/m2", "outcome": "a_future_outcome"},
    ],
    "streams": [{"stream": "INBOX", "phase": "live"}],
}

AUDIT_ENTRY = {
    "id": 1, "at_ms": 9, "action": "capture_withheld", "mob_id": "mob-a",
    "member": {"mob_id": "mob-a", "identity": "research:1"}, "source_id": "mail",
    "generation": 1, "reason": "no_direct_route", "count": 3, "first_at_ms": 5, "last_at_ms": 9,
}

SERVED = [
    ("announce", lambda h: h.announce("ops/1", ["ops.alert"], "Disk full", correlation_id="c-1"),
     "meta/mob_announcements/announce",
     {"source_key": "ops/1", "topics": ["ops.alert"], "body": "Disk full", "correlation_id": "c-1"}),
    ("announcement_get", lambda h: h.announcement_get("mob-a", "ops/1"),
     "meta/mob_announcements/get", {"mob_id": "mob-a", "source_key": "ops/1"}),
    ("announcement_subscription_put",
     lambda h: h.announcement_subscription_put("op-1", "sub-1", MEMBER, ["ops.alert"], mobs=["mob-a"]),
     "meta/mob_announcements/subscriptions/put",
     {"operation_id": "op-1", "subscription_id": "sub-1", "member": MEMBER,
      "topics": ["ops.alert"], "mobs": ["mob-a"]}),
    ("announcement_subscription_pause", lambda h: h.announcement_subscription_pause("op-2", "sub-1", 2),
     "meta/mob_announcements/subscriptions/pause",
     {"operation_id": "op-2", "subscription_id": "sub-1", "expected_revision": 2}),
    ("announcement_subscription_resume", lambda h: h.announcement_subscription_resume("op-3", "sub-1", 3),
     "meta/mob_announcements/subscriptions/resume",
     {"operation_id": "op-3", "subscription_id": "sub-1", "expected_revision": 3}),
    ("announcement_subscription_cancel", lambda h: h.announcement_subscription_cancel("op-4", "sub-1", 4),
     "meta/mob_announcements/subscriptions/cancel",
     {"operation_id": "op-4", "subscription_id": "sub-1", "expected_revision": 4}),
    ("announcement_subscription_get", lambda h: h.announcement_subscription_get("sub-1"),
     "meta/mob_announcements/subscriptions/get", {"subscription_id": "sub-1"}),
    ("announcement_subscription_list", lambda h: h.announcement_subscription_list(member=MEMBER, limit=5),
     "meta/mob_announcements/subscriptions/list", {"member": MEMBER, "limit": 5}),
    ("announcement_delivery_list", lambda h: h.announcement_delivery_list(subscription_id="sub-1"),
     "meta/mob_announcements/deliveries/list", {"subscription_id": "sub-1"}),
    ("announcement_delivery_observe", lambda h: h.announcement_delivery_observe("batch-1"),
     "meta/mob_announcements/deliveries/observe", {"batch_id": "batch-1"}),
    ("announcement_delivery_reconcile", lambda h: h.announcement_delivery_reconcile("batch-1"),
     "meta/mob_announcements/deliveries/reconcile", {"batch_id": "batch-1"}),
    ("announcement_source_put",
     lambda h: h.announcement_source_put(
         "op-5", "mail", SOURCE["identity"], SOURCE["observation"], ["mail.new"],
         SOURCE["audience"], runner={"host_runnable": "mail_poll", "every_seconds": 300}),
     "meta/mob_announcements/sources/put",
     {"operation_id": "op-5", "source_id": "mail", "identity": SOURCE["identity"],
      "observation": SOURCE["observation"], "topics": ["mail.new"], "audience": SOURCE["audience"],
      "runner": {"host_runnable": "mail_poll", "every_seconds": 300}}),
    ("announcement_source_pause", lambda h: h.announcement_source_pause("op-6", "mail", 3),
     "meta/mob_announcements/sources/pause",
     {"operation_id": "op-6", "source_id": "mail", "expected_revision": 3}),
    ("announcement_source_resume",
     lambda h: h.announcement_source_resume("op-7", "mail", 4, rebaseline=True),
     "meta/mob_announcements/sources/resume",
     {"operation_id": "op-7", "source_id": "mail", "expected_revision": 4, "rebaseline": True}),
    ("announcement_source_end", lambda h: h.announcement_source_end("op-8", "mail", 5),
     "meta/mob_announcements/sources/end",
     {"operation_id": "op-8", "source_id": "mail", "expected_revision": 5}),
    ("announcement_source_get", lambda h: h.announcement_source_get("mail", include_streams=True),
     "meta/mob_announcements/sources/get", {"source_id": "mail", "include_streams": True}),
    ("announcement_source_list", lambda h: h.announcement_source_list(state="active", limit=10),
     "meta/mob_announcements/sources/list", {"state": "active", "limit": 10}),
    ("announcement_source_advance",
     lambda h: h.announcement_source_advance(
         "op-9", "mail", 3, 4, SOURCE["identity"]["principal"], "cursor-5",
         items=[{"item_id": "m1"}], observed={"kind": "delta"}),
     "meta/mob_announcements/sources/advance",
     {"operation_id": "op-9", "source_id": "mail", "expected_revision": 3, "expected_version": 4,
      "principal": SOURCE["identity"]["principal"], "next_cursor": "cursor-5",
      "items": [{"item_id": "m1"}], "observed": {"kind": "delta"}}),
    ("announcement_source_record_failure",
     lambda h: h.announcement_source_record_failure(
         "op-10", "mail", 3, 4, SOURCE["identity"]["principal"],
         {"class": "rate_limited", "retry_after_ms": 60000}),
     "meta/mob_announcements/sources/record_failure",
     {"operation_id": "op-10", "source_id": "mail", "expected_revision": 3, "expected_version": 4,
      "principal": SOURCE["identity"]["principal"],
      "failure": {"class": "rate_limited", "retry_after_ms": 60000}}),
    ("announcement_audit_list", lambda h: h.announcement_audit_list(source_id="mail", after=4),
     "meta/mob_announcements/audit/list", {"source_id": "mail", "after": 4}),
]

RESPONSES = {
    "meta/mob_announcements/announce": {"announcement": ANNOUNCEMENT, "replayed": False, "captured": 2},
    "meta/mob_announcements/get": {"announcement": ANNOUNCEMENT},
    "meta/mob_announcements/subscriptions/put": {"subscription": SUBSCRIPTION},
    "meta/mob_announcements/subscriptions/pause": {"subscription": SUBSCRIPTION, "unresolved_batches": []},
    "meta/mob_announcements/subscriptions/resume": {"subscription": SUBSCRIPTION, "unresolved_batches": []},
    "meta/mob_announcements/subscriptions/cancel": {"subscription": SUBSCRIPTION,
                                                    "unresolved_batches": ["batch-1"]},
    "meta/mob_announcements/subscriptions/get": {"subscription": SUBSCRIPTION, "marker": None},
    "meta/mob_announcements/subscriptions/list": {"subscriptions": [SUBSCRIPTION]},
    "meta/mob_announcements/deliveries/list": {"deliveries": [BATCH]},
    "meta/mob_announcements/deliveries/observe": {"batch": BATCH, "native": {"status": "not_committed"}},
    "meta/mob_announcements/deliveries/reconcile": {"batch": BATCH, "native": {"status": "not_committed"}},
    "meta/mob_announcements/sources/put": {"source": SOURCE},
    "meta/mob_announcements/sources/pause": {"source": SOURCE},
    "meta/mob_announcements/sources/resume": {"source": SOURCE},
    "meta/mob_announcements/sources/end": {"source": SOURCE},
    "meta/mob_announcements/sources/get": {"source": SOURCE, "streams": [
        {"stream": "INBOX", "phase": "live", "since_ms": 5, "backfilled": 0}]},
    "meta/mob_announcements/sources/list": {"sources": [SOURCE]},
    "meta/mob_announcements/sources/advance": ADVANCE,
    "meta/mob_announcements/sources/record_failure": {"source": SOURCE},
    "meta/mob_announcements/audit/list": {"entries": [AUDIT_ENTRY]},
}


@pytest.mark.asyncio
@pytest.mark.parametrize("name,call,method,params", SERVED, ids=[row[0] for row in SERVED])
async def test_every_served_method_sends_its_wire_name_and_params(name, call, method, params):
    handle, calls = make_mock_mob_handle(RESPONSES)
    await call(handle)
    assert calls == [(method, params)]


@pytest.mark.asyncio
async def test_announce_reads_the_announcement_and_counts_captures():
    handle, _ = make_mock_mob_handle(RESPONSES)
    result = await handle.announce("ops/1", ["ops.alert"], "Disk full")
    assert result.replayed is False and result.captured == 2
    announcement = result.announcement
    assert (announcement.mob_id, announcement.sequence, announcement.source_key) == ("mob-a", 7, "ops/1")
    assert announcement.announced_by.kind == "host" and announcement.topics == ("ops.alert",)
    assert announcement.causes[0].source_key == "ops/0" and announcement.correlation_id == "c-1"


@pytest.mark.asyncio
async def test_announcement_get_answers_none_for_an_unknown_key():
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/get": {"announcement": None}})
    assert await handle.announcement_get("mob-a", "ops/9") is None


def test_an_unknown_announcer_kind_keeps_its_raw_object():
    from meerkat_meta import Announcement
    raw = {**ANNOUNCEMENT, "announced_by": {"kind": "a_future_announcer", "lane": 3}}
    announcer = Announcement.from_dict(raw).announced_by
    assert announcer.kind == "a_future_announcer" and announcer.raw == {"kind": "a_future_announcer", "lane": 3}
    source = Announcement.from_dict({**ANNOUNCEMENT, "announced_by": {"kind": "source", "source_id": "mail"}})
    assert source.announced_by.source_id == "mail"


@pytest.mark.asyncio
async def test_subscription_records_keep_open_state_and_frontier():
    handle, _ = make_mock_mob_handle(
        {"meta/mob_announcements/subscriptions/list": {"subscriptions": [
            {**SUBSCRIPTION, "state": "a_future_state"}]}})
    [subscription] = await handle.announcement_subscription_list()
    assert subscription.state == "a_future_state" and subscription.frontier == {"mob-a": 6}
    assert subscription.member.identity == "lead:main" and subscription.topics == ("ops.alert",)
    assert subscription.mobs == ("mob-a",) and subscription.revision == 2


@pytest.mark.asyncio
async def test_subscription_change_reads_unresolved_batches():
    handle, _ = make_mock_mob_handle(RESPONSES)
    change = await handle.announcement_subscription_cancel("op-4", "sub-1", 4)
    assert change.subscription.id == "sub-1" and change.unresolved_batches == ("batch-1",)


@pytest.mark.asyncio
async def test_subscription_get_reads_the_marker_and_an_absent_subscription():
    marker = {"cause": {"kind": "peer_composing"}, "evidence": "observed", "since_ms": 5}
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/subscriptions/get": {
        "subscription": SUBSCRIPTION, "marker": marker,
        "backlog": {"eligible_count": 3, "outstanding": True}}})
    view = await handle.announcement_subscription_get("sub-1")
    assert view.subscription.id == "sub-1" and view.marker.cause_kind == "peer_composing"
    assert view.marker.since_ms == 5 and view.backlog == {"eligible_count": 3, "outstanding": True}
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/subscriptions/get": {
        "subscription": None, "marker": None}})
    view = await handle.announcement_subscription_get("sub-9")
    assert view.subscription is None and view.marker is None and view.backlog is None


@pytest.mark.asyncio
async def test_deliveries_read_batch_state_handling_and_gaps():
    handle, _ = make_mock_mob_handle(RESPONSES)
    [batch] = await handle.announcement_delivery_list()
    assert batch.id == "batch-1" and batch.state == "delivered"
    assert batch.state_detail == {"state": "delivered", "receipt": {"delivery_id": "d-1"}}
    assert batch.handling.state == "handled" and batch.route == "local"
    assert batch.items[0].sequence == 7 and batch.gaps[0]["reason"] == "expired"


@pytest.mark.asyncio
async def test_delivery_observe_reads_the_observation():
    handle, _ = make_mock_mob_handle(RESPONSES)
    observation = await handle.announcement_delivery_observe("batch-1")
    assert observation.batch.id == "batch-1" and observation.native == {"status": "not_committed"}
    assert observation.native_status == "not_committed"
    assert observation.marker is None


@pytest.mark.asyncio
async def test_source_record_reads_typed_fields_and_keeps_vocabulary_open():
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/sources/list": {"sources": [
        {**SOURCE, "state": "a_future_state", "collection": "a_future_phase",
         "health": {**SOURCE["health"], "status": "a_future_status"}}]}})
    [source] = await handle.announcement_source_list()
    assert (source.source_id, source.generation, source.revision) == ("mail", 1, 3)
    assert source.state == "a_future_state" and source.collection == "a_future_phase"
    assert source.health.status == "a_future_status" and source.health.totals["announced"] == 5
    assert source.cursor.version == 4 and source.cursor.token == "opaque-token"
    assert source.runner.host_runnable == "mail_poll" and source.runner.every_seconds == 300
    assert source.identity.principal["kind"] == "connector_verified_account"
    assert [route.kind for route in source.routes] == ["direct", "a_future_route"]
    assert source.routes[1].raw == {"kind": "a_future_route", "lane": 2}
    assert source.topics == ("mail.new",) and source.streams == {"live": 1, "onboarding": 0}


@pytest.mark.asyncio
async def test_source_put_reads_affected_subscriptions_and_delivery_readiness():
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/sources/put": {
        "source": SOURCE, "affected_subscriptions": 4, "delivery_readiness": {"kind": "ready"}}})
    put = await handle.announcement_source_put(
        "op-5", "mail", SOURCE["identity"], SOURCE["observation"], ["mail.new"], SOURCE["audience"])
    assert put.source.source_id == "mail" and put.affected_subscriptions == 4
    assert put.delivery_readiness == {"kind": "ready"}
    handle, _ = make_mock_mob_handle(RESPONSES)
    put = await handle.announcement_source_put(
        "op-5", "mail", SOURCE["identity"], SOURCE["observation"], ["mail.new"], SOURCE["audience"])
    assert put.affected_subscriptions is None and put.delivery_readiness is None


@pytest.mark.asyncio
async def test_source_get_reads_streams_when_asked():
    handle, _ = make_mock_mob_handle(RESPONSES)
    view = await handle.announcement_source_get("mail", include_streams=True)
    assert view.source.source_id == "mail" and view.streams[0].phase == "live"
    assert view.streams[0].since_ms == 5


@pytest.mark.asyncio
async def test_source_get_answers_none_for_an_unknown_source():
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/sources/get": {"source": None}})
    view = await handle.announcement_source_get("no-such-source")
    assert view.source is None and view.streams is None


@pytest.mark.asyncio
async def test_source_advance_reads_item_outcomes_and_streams():
    handle, _ = make_mock_mob_handle(RESPONSES)
    advance = await handle.announcement_source_advance(
        "op-9", "mail", 3, 4, SOURCE["identity"]["principal"], "cursor-5")
    assert advance.replayed is False and advance.source.revision == 3
    first, second = advance.items
    assert (first.outcome, first.sequence, first.captured) == ("announced", 8, 2)
    assert second.outcome == "a_future_outcome" and second.sequence is None
    assert advance.streams[0].stream == "INBOX" and advance.streams[0].phase == "live"


@pytest.mark.asyncio
async def test_audit_list_reads_counted_entries():
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/audit/list": {"entries": [
        AUDIT_ENTRY, {**AUDIT_ENTRY, "id": 2, "action": "a_future_action"}]}})
    first, second = await handle.announcement_audit_list()
    assert (first.id, first.action, first.reason, first.count) == (1, "capture_withheld", "no_direct_route", 3)
    assert first.member.identity == "research:1" and second.action == "a_future_action"


@pytest.mark.asyncio
async def test_an_audit_entry_for_a_plain_announcement_names_no_source():
    plain = {key: value for key, value in AUDIT_ENTRY.items() if key not in ("source_id", "generation")}
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/audit/list": {"entries": [
        {**plain, "action": "redelivered", "reason": "unhandled"}, AUDIT_ENTRY]}})
    first, second = await handle.announcement_audit_list()
    assert (first.action, first.reason, first.source_id, first.generation) == (
        "redelivered", "unhandled", None, None)
    assert (second.source_id, second.generation) == ("mail", 1)


def _error(code, kind=None, **data):
    from meerkat_meta.runtime import _rpc_error_from_payload
    payload = {"code": code, "message": "refused"}
    if kind is not None:
        payload["data"] = {"kind": kind, **data}
    return _rpc_error_from_payload(payload, request_id="r-1", method="meta/mob_announcements/sources/put")


@pytest.mark.parametrize("code,kind,cls_name", [
    (-32044, "mob_announcements_unavailable", "MobAnnouncementsUnavailableError"),
    (-32045, "operation_reused", "AnnouncementConflictError"),
    (-32046, "gap", "AnnouncementFrontierError"),
    (-32047, "topic_owned", "AnnouncementRefusedError"),
    (-32048, "unknown_source", "AnnouncementNotFoundError"),
])
def test_announcement_error_codes_map_to_typed_errors(code, kind, cls_name):
    import meerkat_meta
    error = _error(code, kind, topic="mail.new", source_id="mail")
    assert type(error) is getattr(meerkat_meta, cls_name)
    assert isinstance(error, RpcError) and error.code == code and error.kind == kind
    assert error.data["kind"] == kind


def test_source_conflicts_carry_the_current_position():
    from meerkat_meta import (AnnouncementConflictError, SourceStaleRevisionError,
                              SourceVersionConflictError)
    cursor = {"version": 9, "token": "t", "advanced_at_ms": 1}
    stale = _error(-32045, "stale_revision", current_revision=5, current_version=9, generation=2,
                   cursor=cursor)
    assert type(stale) is SourceStaleRevisionError and isinstance(stale, AnnouncementConflictError)
    assert (stale.current_revision, stale.current_version, stale.generation) == (5, 9, 2)
    assert stale.cursor == cursor
    conflict = _error(-32045, "source_version_conflict", current_version=9, cursor=cursor,
                      state="active", revision=5)
    assert type(conflict) is SourceVersionConflictError
    assert (conflict.current_version, conflict.revision, conflict.state) == (9, 5, "active")
    # A subscription's stale revision has no source position: the family base.
    plain = _error(-32045, "stale_revision", current_revision=3)
    assert type(plain) is AnnouncementConflictError and plain.kind == "stale_revision"


def test_an_unknown_refusal_kind_keeps_the_family_base_with_its_kind():
    from meerkat_meta import AnnouncementRefusedError
    error = _error(-32047, "a_future_refusal", detail=1)
    assert type(error) is AnnouncementRefusedError and error.kind == "a_future_refusal"


def test_an_unrelated_error_code_is_not_an_announcement_error():
    from meerkat_meta import AnnouncementConflictError
    assert not isinstance(_error(-32042, "stale_revision"), AnnouncementConflictError)


def _failing_handle(error):
    from meerkat_meta.runtime import MobHandle
    from unittest.mock import MagicMock

    async def failing(method, params=None, *, transport_timeout=None):
        raise error(method)

    runtime = MagicMock()
    runtime._rpc = failing
    handle = MobHandle.__new__(MobHandle)
    handle._runtime = runtime
    return handle


@pytest.mark.asyncio
async def test_a_gateway_conflict_reaches_the_caller_as_its_typed_class():
    from meerkat_meta import SourceStaleRevisionError
    handle = _failing_handle(lambda method: _error(
        -32045, "stale_revision", current_revision=5, current_version=9, generation=2, cursor=None))
    with pytest.raises(SourceStaleRevisionError) as caught:
        await handle.announcement_source_advance("op-9", "mail", 3, 4, SOURCE["identity"]["principal"],
                                                 "cursor-5")
    assert caught.value.current_revision == 5


def _method_not_found(method):
    return RpcError(code=-32601, message="Method not found", request_id="r-1", method=method)


@pytest.mark.asyncio
@pytest.mark.parametrize("call", [
    lambda handle: handle.announcement_source_get("mail"),
    lambda handle: handle.announcement_audit_list(),
], ids=["sources/get", "audit/list"])
async def test_a_sources_method_on_an_older_meta_says_sources_are_unsupported(call):
    with pytest.raises(RpcError) as caught:
        await call(_failing_handle(_method_not_found))
    assert caught.value.code == -32601 and "Meta does not support sources" in str(caught.value)


@pytest.mark.asyncio
async def test_a_subscription_method_on_an_older_meta_keeps_the_plain_error():
    with pytest.raises(RpcError) as caught:
        await _failing_handle(_method_not_found).announcement_subscription_get("sub-1")
    assert caught.value.code == -32601 and "sources" not in str(caught.value)


def test_capabilities_detect_sources_and_read_the_announcements_object():
    from meerkat_meta import CapabilitiesResult, supports_announcement_sources
    data = {"contract_version": "0.5.0", "methods": ["meta/mob_announcements/announce"],
            "loaded_modules": [], "feature_capabilities": ["mob_announcements.sources.v1"],
            "mob_announcements": {"sources": "v1",
                                  "local_delivery": {"kind": "refused", "cause": "governed_runtime"},
                                  "governed_admission": False}}
    capabilities = CapabilitiesResult.from_dict(data)
    assert supports_announcement_sources(capabilities) is True
    assert supports_announcement_sources(["mob_announcements.sources.v1"]) is True
    announcements = capabilities.mob_announcements
    assert announcements.sources == "v1" and announcements.governed_admission is False
    assert announcements.local_delivery == {"kind": "refused", "cause": "governed_runtime"}
    assert announcements.local_delivery_kind == "refused"
    older = CapabilitiesResult.from_dict({**data, "feature_capabilities": []})
    assert supports_announcement_sources(older) is False
    disabled = CapabilitiesResult.from_dict({**data, "mob_announcements": None})
    assert disabled.mob_announcements is None
    absent = {key: value for key, value in data.items() if key != "mob_announcements"}
    assert CapabilitiesResult.from_dict(absent).mob_announcements is None


def test_an_escalation_error_event_passes_through_before_the_sdk_names_it():
    event = ErrorEvent.from_dict({"category": "mob_announcement_escalated",
                                  "escalation_id": "e-1", "cause": "source_failing"})
    assert event.category == "mob_announcement_escalated"
    assert event.context == {"escalation_id": "e-1", "cause": "source_failing"}


@pytest.mark.parametrize("name", [
    "Announcement", "AnnounceResult", "AnnouncementSubscription", "AnnouncementBatch",
    "AnnouncementSource", "SourceAdvanceResult", "AnnouncementAuditEntry",
    "MobAnnouncementsCapability", "supports_announcement_sources",
    "MobAnnouncementsUnavailableError", "AnnouncementConflictError", "SourceVersionConflictError",
    "SourceStaleRevisionError", "AnnouncementRefusedError", "AnnouncementNotFoundError",
    "AnnouncementFrontierError", "MOB_ANNOUNCEMENTS_UNAVAILABLE_CODE", "ANNOUNCEMENT_CONFLICT_CODE",
    "ANNOUNCEMENT_FRONTIER_CODE", "ANNOUNCEMENT_REFUSED_CODE", "ANNOUNCEMENT_NOT_FOUND_CODE",
])
def test_announcement_names_are_exported(name):
    import meerkat_meta
    assert hasattr(meerkat_meta, name) and name in meerkat_meta.__all__


def test_announcement_error_codes_are_the_gateway_codes():
    import meerkat_meta
    codes = [meerkat_meta.MOB_ANNOUNCEMENTS_UNAVAILABLE_CODE, meerkat_meta.ANNOUNCEMENT_CONFLICT_CODE,
             meerkat_meta.ANNOUNCEMENT_FRONTIER_CODE, meerkat_meta.ANNOUNCEMENT_REFUSED_CODE,
             meerkat_meta.ANNOUNCEMENT_NOT_FOUND_CODE]
    assert codes == [-32044, -32045, -32046, -32047, -32048]


def _init_runtime_options(builder):
    from meerkat_meta.runtime import MetaRuntime
    return MetaRuntime(builder._config)._build_init_params()["runtime_options"]


def test_the_builder_leaves_mob_announcements_unset_by_default():
    from meerkat_meta.builder import Meta
    assert "mob_announcements" not in _init_runtime_options(Meta.builder())


@pytest.mark.parametrize("call,sent", [
    (lambda b: b.mob_announcements(), {"enabled": True}),
    (lambda b: b.mob_announcements(False), {"enabled": False}),
    (lambda b: b.mob_announcements({"enabled": True, "retention_ms": 3600000, "batch": {"max_count": 10}}),
     {"enabled": True, "retention_ms": 3600000, "batch": {"max_count": 10}}),
], ids=["enabled", "disabled", "object"])
def test_the_builder_sets_runtime_options_mob_announcements(call, sent):
    from meerkat_meta.builder import Meta
    assert _init_runtime_options(call(Meta.builder()))["mob_announcements"] == sent


# -- A real rpc_gateway ------------------------------------------------------
#
# The rows above pin the SDK's reading of the wire. This one runs the SDK
# against the gateway itself, so the two cannot drift apart unnoticed. It
# needs a built rpc_gateway (``make test-python`` builds it) and makes no
# model call: the member is turn driven and nothing is delivered to it.

def _gateway_bin() -> str:
    import os
    import subprocess
    from pathlib import Path

    override = os.environ.get("META_GATEWAY_BIN", "").strip()
    if override:
        return override
    repo_root = Path(__file__).resolve().parents[3]
    try:
        result = subprocess.run(
            [str(repo_root / "scripts" / "repo-cargo"), "--print-env", "CARGO_TARGET_DIR"],
            cwd=repo_root, check=True, capture_output=True, text=True,
        )
    except (OSError, subprocess.CalledProcessError):
        return ""
    for line in result.stdout.splitlines():
        if line.startswith("CARGO_TARGET_DIR="):
            target_dir = line.split("=", 1)[1].strip()
            if target_dir:
                return str(Path(target_dir) / "debug" / "rpc_gateway")
    return ""


_GATEWAY_BIN = _gateway_bin()
_ROUND_TRIP_MOB_ID = "announcement-sources-round-trip"
_ROUND_TRIP_MOB = f"""\
[mob]
id = "{_ROUND_TRIP_MOB_ID}"

[profiles.reader]
model = "gpt-5.5"
external_addressable = true

[profiles.reader.tools]
comms = true
"""


class _ReaderRoster:
    async def roster(self, context):
        from meerkat_meta.identity_first_models import DurableAgentSpec
        return [DurableAgentSpec(identity="reader:1", profile="reader", addressability="addressable",
                                 runtime_mode_override="turn_driven")]


def _gateway_present() -> bool:
    import os
    return bool(_GATEWAY_BIN) and os.path.isfile(_GATEWAY_BIN)


@pytest.mark.asyncio
@pytest.mark.skipif(not _gateway_present(), reason="no built rpc_gateway (make test-python builds it)")
async def test_announcement_sources_round_trip(tmp_path):
    from meerkat_meta import (AnnouncementNotFoundError, AnnouncementRefusedError, RpcError,
                              SourceStaleRevisionError, SourceVersionConflictError)
    from meerkat_meta.builder import Meta

    mob_toml = tmp_path / "mob.toml"
    mob_toml.write_text(_ROUND_TRIP_MOB)
    runtime = await (
        Meta.builder()
        .gateway(_GATEWAY_BIN)
        .mob(str(mob_toml))
        .persistent_state(str(tmp_path / "state"))
        .roster(_ReaderRoster())
        .mob_announcements()
        .build()
    )
    try:
        handle = runtime.mob_handle()
        capabilities = await handle.capabilities()
        assert "meta/mob_announcements/sources/advance" in capabilities.methods

        first = await handle.announce("ops/1", ["ops.alert"], "Disk full on db-2", correlation_id="c-1")
        assert first.replayed is False and first.announcement.announced_by.kind == "host"
        again = await handle.announce("ops/1", ["ops.alert"], "Disk full on db-2", correlation_id="c-1")
        assert again.replayed is True and again.announcement.sequence == first.announcement.sequence
        stored = await handle.announcement_get(_ROUND_TRIP_MOB_ID, "ops/1")
        assert stored.body == "Disk full on db-2" and stored.correlation_id == "c-1"
        assert await handle.announcement_get(_ROUND_TRIP_MOB_ID, "ops/9") is None

        member = {"mob_id": _ROUND_TRIP_MOB_ID, "identity": "reader:1"}
        principal = {"kind": "host_attested", "attester": "round-trip-host", "account": "reader@example.test"}
        identity = {"provider": "example.mail", "principal": principal, "scope": {"kind": "inbox", "id": "INBOX"}}
        put = await handle.announcement_source_put(
            "op-source", "mail", identity, {"baseline": "delta", "deletions": "explicit"}, ["mail.new"],
            [{"kind": "member", "member": member}], connection={"kind": "host", "id": "conn-1"})
        source = put.source
        assert (source.source_id, source.generation, source.state, source.collection) == (
            "mail", 1, "active", "baselining")
        assert source.cursor.version == 0 and [route.kind for route in source.routes] == ["direct"]
        assert source.identity.principal == principal

        subscription = await handle.announcement_subscription_put("op-sub", "sub-1", member, ["mail.new"])
        assert subscription.state == "active" and subscription.member.identity == "reader:1"
        assert subscription.topics == ("mail.new",) and subscription.mobs == (_ROUND_TRIP_MOB_ID,)
        view = await handle.announcement_subscription_get("sub-1")
        assert view.subscription.id == "sub-1"
        [listed_subscription] = await handle.announcement_subscription_list(member=member)
        assert listed_subscription.id == "sub-1"

        baseline = await handle.announcement_source_advance(
            "op-step-0", "mail", source.revision, 0, principal, "cursor-1", coverage="complete",
            observed={"kind": "delta"})
        assert baseline.replayed is False and baseline.items == ()
        assert baseline.source.collection == "live" and baseline.source.cursor.version == 1

        item = {"item_id": "m1", "version": "v1", "change": "upsert", "topics": ["mail.new"],
                "body": "A new message in the inbox"}
        step = await handle.announcement_source_advance(
            "op-step-1", "mail", baseline.source.revision, 1, principal, "cursor-2", items=[item])
        [outcome] = step.items
        assert (outcome.item_id, outcome.outcome, outcome.captured) == ("m1", "announced", 1)
        assert outcome.source_key.startswith("source:mail:") and outcome.sequence is not None
        assert step.source.cursor.version == 2 and step.source.health.totals["announced"] == 1
        announced = await handle.announcement_get(_ROUND_TRIP_MOB_ID, outcome.source_key)
        assert announced.announced_by.kind == "source" and announced.announced_by.source_id == "mail"

        replay = await handle.announcement_source_advance(
            "op-step-1", "mail", baseline.source.revision, 1, principal, "cursor-2", items=[item])
        assert replay.replayed is True and replay.items[0].outcome == "announced"

        with pytest.raises(SourceVersionConflictError) as conflict:
            await handle.announcement_source_advance(
                "op-step-late", "mail", step.source.revision, 1, principal, "cursor-x")
        assert conflict.value.current_version == 2 and conflict.value.state == "active"
        assert conflict.value.cursor["token"] == "cursor-2"
        with pytest.raises(SourceStaleRevisionError) as stale:
            await handle.announcement_source_advance(
                "op-step-stale", "mail", step.source.revision + 7, 2, principal, "cursor-x")
        assert stale.value.current_revision == step.source.revision and stale.value.current_version == 2
        assert stale.value.generation == 1

        failed = await handle.announcement_source_record_failure(
            "op-failure", "mail", step.source.revision, 2, principal,
            {"class": "rate_limited", "retry_after_ms": 60000, "detail": "provider said slow down"})
        assert failed.health.consecutive_failures == 1
        assert failed.health.last_failure["class"] == "rate_limited"

        got = await handle.announcement_source_get("mail", include_streams=True)
        assert got.source.cursor.token == "cursor-2" and got.streams == ()
        assert got.source.health.last_failure["detail"] == "provider said slow down"
        [listed] = await handle.announcement_source_list()
        assert listed.cursor.token is None and set(listed.identity.principal) == {"kind", "digest"}
        assert "detail" not in listed.health.last_failure
        assert await handle.announcement_audit_list(source_id="mail") == []
        deliveries = await handle.announcement_delivery_list(subscription_id="sub-1")
        assert all(batch.member.identity == "reader:1" for batch in deliveries)

        paused = await handle.announcement_source_pause("op-pause", "mail", got.source.revision)
        assert paused.state == "paused"
        with pytest.raises(AnnouncementRefusedError) as refused:
            await handle.announcement_source_advance(
                "op-step-paused", "mail", paused.revision, 2, principal, "cursor-3")
        assert refused.value.kind == "source_paused"
        resumed = await handle.announcement_source_resume("op-resume", "mail", paused.revision)
        assert resumed.state == "active" and resumed.cursor.version == 2
        ended = await handle.announcement_source_end("op-end", "mail", resumed.revision)
        assert ended.state == "ended"

        assert (await handle.announcement_source_get("no-such-source")).source is None
        with pytest.raises(AnnouncementNotFoundError) as missing:
            await handle.announcement_source_pause("op-pause-missing", "no-such-source", 1)
        assert missing.value.kind == "unknown_source"
        for held_remedy, operation_id in ((handle.announcement_delivery_redeliver, "op-redeliver-missing"),
                                          (handle.announcement_delivery_dismiss, "op-dismiss-missing")):
            with pytest.raises(AnnouncementNotFoundError) as unknown_batch:
                await held_remedy(operation_id, "no-such-batch")
            assert unknown_batch.value.kind == "unknown_delivery"

        nothing_declared = await handle.announcement_declarations_get()
        assert (nothing_declared.revision, nothing_declared.records) == (0, ())
        with pytest.raises(RpcError) as no_sidecar:
            await handle.announcement_declarations_apply()
        assert no_sidecar.value.code == -32602
        declared = await handle.announcement_declarations_apply({"revision": 1, "subscriptions": [
            {"subscription_id": "declared-1", "member": member, "topics": ["ops.alert"]}]})
        assert declared.revision == 1 and declared.digest.startswith("sha256:")
        [record] = declared.records
        assert (record.kind, record.id, record.status) == ("subscription", "declared-1", "applied")
        assert (await handle.announcement_declarations_get()).digest == declared.digest
        with pytest.raises(AnnouncementRefusedError) as rollback:
            await handle.announcement_declarations_apply({"revision": 0})
        assert rollback.value.kind == "declaration_rollback" and rollback.value.data["applied"] == 1

        readers = {"mob_id": _ROUND_TRIP_MOB_ID, "role": "reader"}
        template = await handle.announcement_role_template_put("op-template", "readers", readers, ["ops.alert"])
        assert (template.id, template.state, template.members.role) == ("readers", "active", "reader")
        assert template.topics == ("ops.alert",) and template.mobs == (_ROUND_TRIP_MOB_ID,)
        viewed = await handle.announcement_subscription_get("readers")
        assert viewed.subscription is None and viewed.template.revision == template.revision
        with pytest.raises(AnnouncementRefusedError) as taken:
            await handle.announcement_subscription_put("op-taken", "readers", member, ["ops.alert"])
        assert taken.value.kind == "subscription_id_taken"
        await handle.announce("ops/2", ["ops.alert"], "Disk full on db-3")
        paused_template = await handle.announcement_subscription_pause(
            "op-template-pause", "readers", template.revision)
        assert paused_template.subscription is None and paused_template.template.state == "paused"
        [child] = paused_template.children
        assert child.id.startswith("readers/") and child.member.identity == "reader:1"
        cancelled_template = await handle.announcement_subscription_cancel(
            "op-template-cancel", "readers", paused_template.template.revision)
        assert cancelled_template.template.state == "cancelled"
        assert [c.state for c in cancelled_template.children] == ["cancelled"]
        cancelled = await handle.announcement_subscription_cancel(
            "op-cancel", "sub-1", listed_subscription.revision)
        assert cancelled.subscription.state == "cancelled"
    finally:
        await runtime.shutdown()


# -- Handling, redelivery and dismissal ---------------------------------------

HANDLING = {"state": "unhandled", "cause": "abandoned_with_error", "redelivered": 0, "held": 2,
            "dismissed": 0, "read_at_ms": 50}

HANDLING_SERVED = [
    ("announcement_delivery_redeliver", lambda h: h.announcement_delivery_redeliver("op-r", "batch-1"),
     "meta/mob_announcements/deliveries/redeliver", {"operation_id": "op-r", "batch_id": "batch-1"}),
    ("announcement_delivery_dismiss", lambda h: h.announcement_delivery_dismiss("op-d", "batch-1"),
     "meta/mob_announcements/deliveries/dismiss", {"operation_id": "op-d", "batch_id": "batch-1"}),
]


@pytest.mark.asyncio
@pytest.mark.parametrize("name,call,method,params", HANDLING_SERVED, ids=[row[0] for row in HANDLING_SERVED])
async def test_every_handling_method_sends_its_wire_name_and_params(name, call, method, params):
    handle, calls = make_mock_mob_handle()
    await call(handle)
    assert calls == [(method, params)]


@pytest.mark.asyncio
async def test_redeliver_and_dismiss_read_the_handling():
    handle, _ = make_mock_mob_handle({
        "meta/mob_announcements/deliveries/redeliver": {"handling": {**HANDLING, "held": 0, "redelivered": 2}},
        "meta/mob_announcements/deliveries/dismiss": {"handling": {**HANDLING, "held": 0, "dismissed": 2}}})
    redelivered = await handle.announcement_delivery_redeliver("op-r", "batch-1")
    assert (redelivered.state, redelivered.cause, redelivered.redelivered, redelivered.held) == (
        "unhandled", "abandoned_with_error", 2, 0)
    dismissed = await handle.announcement_delivery_dismiss("op-d", "batch-1")
    assert (dismissed.dismissed, dismissed.read_at_ms) == (2, 50)


@pytest.mark.asyncio
async def test_a_batch_and_its_observation_read_the_handling():
    handle, _ = make_mock_mob_handle({
        "meta/mob_announcements/deliveries/list": {"deliveries": [
            {**BATCH, "handling": HANDLING}, {**BATCH, "handling": None},
            {**BATCH, "handling": {**HANDLING, "state": "a_future_state", "cause": None}}]},
        "meta/mob_announcements/deliveries/observe": {"batch": BATCH, "native": {"status": "applied"},
                                                      "handling": {**HANDLING, "state": "pending"}}})
    handled, unread, future = await handle.announcement_delivery_list()
    assert (handled.handling.state, handled.handling.held) == ("unhandled", 2)
    assert unread.handling is None
    assert future.handling.state == "a_future_state" and future.handling.cause is None
    observation = await handle.announcement_delivery_observe("batch-1")
    assert observation.handling.state == "pending"
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/deliveries/observe": {
        "batch": BATCH, "native": {"status": "not_submitted"}}})
    assert (await handle.announcement_delivery_observe("batch-1")).handling is None


def test_the_handling_name_is_exported():
    import meerkat_meta
    assert "AnnouncementBatchHandling" in meerkat_meta.__all__


# -- Members, escalations and status ----------------------------------------

ESCALATION = {
    "escalation_id": "esc-1", "cause": "failure_streak",
    "subject": {"kind": "source", "mob_id": "mob-a", "source_id": "mail"},
    "since_ms": 10, "raised_at_ms": 20, "renotify_count": 1,
}

MEMBER_READ = {
    "member": MEMBER,
    "backlog": {"eligible_count": 3, "eligible_bytes": 120, "oldest_captured_at_ms": 5,
                "last_sealed_at_ms": None, "outstanding": True, "held_audience": 1,
                "held_unhandled": 0, "held_refused": 0, "withheld_audience": 2},
    "marker": {"cause": {"kind": "member_not_live"}, "evidence": "local", "since_ms": 7},
    "escalations": [{**ESCALATION, "escalation_id": "esc-2", "cause": "suspended",
                     "subject": {"kind": "member", "member": MEMBER}}],
}

ESCALATION_EVENT = {
    "cursor": 4, "escalation_id": "esc-1", "event": "raised", "cause": "failure_streak",
    "subject": ESCALATION["subject"], "since_ms": 10, "at_ms": 20, "renotify_count": 0,
}

HEALTH_SERVED = [
    ("announcement_member_get", lambda h: h.announcement_member_get(MEMBER),
     "meta/mob_announcements/members/get", {"member": MEMBER}),
    ("announcement_member_list", lambda h: h.announcement_member_list(after=MEMBER, limit=5, escalated=True),
     "meta/mob_announcements/members/list", {"after": MEMBER, "limit": 5, "escalated": True}),
    ("announcement_escalations_list", lambda h: h.announcement_escalations_list(after=3, limit=10),
     "meta/mob_announcements/escalations/list", {"after": 3, "limit": 10}),
    ("announcement_escalations_list[open]", lambda h: h.announcement_escalations_list(open=True),
     "meta/mob_announcements/escalations/list", {"open": True}),
    ("announcement_source_list[escalated]", lambda h: h.announcement_source_list(escalated=True),
     "meta/mob_announcements/sources/list", {"escalated": True}),
]


@pytest.mark.asyncio
@pytest.mark.parametrize("name,call,method,params", HEALTH_SERVED, ids=[row[0] for row in HEALTH_SERVED])
async def test_every_health_method_sends_its_wire_name_and_params(name, call, method, params):
    handle, calls = make_mock_mob_handle()
    await call(handle)
    assert calls == [(method, params)]


@pytest.mark.asyncio
async def test_member_get_reads_backlog_marker_and_escalations():
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/members/get": MEMBER_READ})
    view = await handle.announcement_member_get(MEMBER)
    assert view.member.identity == "lead:main" and view.marker.cause_kind == "member_not_live"
    assert (view.backlog.eligible_count, view.backlog.eligible_bytes, view.backlog.outstanding) == (3, 120, True)
    assert (view.backlog.held_audience, view.backlog.withheld_audience) == (1, 2)
    assert view.backlog.last_sealed_at_ms is None and view.backlog.oldest_captured_at_ms == 5
    [escalation] = view.escalations
    assert (escalation.escalation_id, escalation.cause, escalation.subject_kind) == ("esc-2", "suspended", "member")


@pytest.mark.asyncio
async def test_member_list_reads_member_views():
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/members/list": {"members": [
        MEMBER_READ, {**MEMBER_READ, "marker": None, "escalations": []}]}})
    first, second = await handle.announcement_member_list()
    assert first.escalations[0].cause == "suspended" and second.marker is None and second.escalations == ()


@pytest.mark.asyncio
async def test_escalations_list_reads_events_or_the_open_escalations():
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/escalations/list": {
        "events": [ESCALATION_EVENT, {**ESCALATION_EVENT, "cursor": 5, "event": "a_future_event"}], "next": 5}})
    page = await handle.announcement_escalations_list(after=3)
    assert page.next == 5 and page.escalations is None
    assert [(event.cursor, event.event) for event in page.events] == [(4, "raised"), (5, "a_future_event")]
    assert page.events[0].subject_kind == "source" and page.events[0].subject["source_id"] == "mail"
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/escalations/list": {
        "escalations": [ESCALATION, {**ESCALATION, "cause": "a_future_cause", "resolved_at_ms": 30}], "next": 9}})
    page = await handle.announcement_escalations_list(open=True)
    assert page.next == 9 and page.events is None
    first, second = page.escalations
    assert (first.cause, first.renotify_count, first.resolved_at_ms) == ("failure_streak", 1, None)
    assert (second.cause, second.resolved_at_ms) == ("a_future_cause", 30)


@pytest.mark.asyncio
async def test_a_source_health_reads_its_escalations_and_monitoring():
    health = {**SOURCE["health"], "escalations": [ESCALATION], "runner_scheduled_here": False,
              "quiet_monitoring": {"configured": True, "quiet_after_ms": 3600000, "last_new_item_at_ms": 40},
              "empty_coverage_monitoring": {"configured": False, "empty_after_ms": None,
                                            "empty_snapshot_since_ms": None}}
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/sources/list": {"sources": [
        {**SOURCE, "health": health}, SOURCE]}})
    escalated, plain = await handle.announcement_source_list()
    assert escalated.health.escalations[0].cause == "failure_streak"
    assert escalated.health.runner_scheduled_here is False
    assert escalated.health.quiet_monitoring["configured"] is True
    assert escalated.health.empty_coverage_monitoring["configured"] is False
    assert plain.health.escalations == () and plain.health.runner_scheduled_here is None
    assert plain.health.quiet_monitoring is None


def test_status_reads_the_mob_announcement_counts():
    from meerkat_meta import StatusResult
    data = {"contract_version": "0.5.0", "running": True, "loaded_modules": [],
            "mob_announcements": {"sources": {"active": 2, "paused": 1, "failing": 1, "stale": 0},
                                  "members": {"suspended": 1, "unhandled_held": 0},
                                  "escalations": {"open": 3, "last_event_cursor": 12}}}
    counts = StatusResult.from_dict(data).mob_announcements
    assert counts.sources == {"active": 2, "paused": 1, "failing": 1, "stale": 0}
    assert counts.members == {"suspended": 1, "unhandled_held": 0}
    assert (counts.open_escalations, counts.last_event_cursor) == (3, 12)
    assert StatusResult.from_dict({**data, "mob_announcements": None}).mob_announcements is None
    absent = {key: value for key, value in data.items() if key != "mob_announcements"}
    assert StatusResult.from_dict(absent).mob_announcements is None


@pytest.mark.parametrize("name", [
    "AnnouncementEscalation", "AnnouncementEscalationEvent", "AnnouncementEscalationsPage",
    "AnnouncementMemberBacklog", "AnnouncementMemberView", "MobAnnouncementsStatus",
])
def test_health_names_are_exported(name):
    import meerkat_meta
    assert hasattr(meerkat_meta, name) and name in meerkat_meta.__all__


# -- Declarations and role templates ------------------------------------------

ROLE = {"mob_id": "mob-a", "role": "reviewer"}

TEMPLATE = {"id": "reviewers", "members": ROLE, "topics": ["ops.alert"], "mobs": ["mob-a"], "state": "active",
            "revision": 1, "created_at_ms": 10, "updated_at_ms": 10}

DECLARATIONS = {"revision": 3, "subscriptions": [
    {"subscription_id": "reviewers", "members": ROLE, "topics": ["ops.alert"]}]}

DECLARATIONS_VIEW = {"revision": 3, "digest": "sha256:abc", "records": [
    {"kind": "source", "id": "mail", "status": "applied", "collection": "live"},
    {"kind": "subscription", "id": "sub-1", "status": "pending",
     "refusal": {"kind": "unknown_member", "message": "no such member"}},
    {"kind": "role_child", "id": "reviewers/0123456789abcdef", "status": "applied",
     "delivery": {"kind": "refused", "cause": "governed_runtime"}},
    {"kind": "a_future_kind", "id": "x", "status": "a_future_status"}]}

DECLARATION_SERVED = [
    ("role_template_put", lambda h: h.announcement_role_template_put("op-t", "reviewers", ROLE, ["ops.alert"]),
     "meta/mob_announcements/subscriptions/put",
     {"operation_id": "op-t", "subscription_id": "reviewers", "members": ROLE, "topics": ["ops.alert"]}),
    ("role_template_put_with_options", lambda h: h.announcement_role_template_put(
        "op-t", "reviewers", ROLE, ["ops.alert"], mobs=["mob-a"], expected_revision=2),
     "meta/mob_announcements/subscriptions/put",
     {"operation_id": "op-t", "subscription_id": "reviewers", "members": ROLE, "topics": ["ops.alert"],
      "mobs": ["mob-a"], "expected_revision": 2}),
    ("declarations_apply", lambda h: h.announcement_declarations_apply(DECLARATIONS),
     "meta/mob_announcements/declarations/apply", {"declarations": DECLARATIONS, "revision": 3}),
    ("declarations_apply_from_a_sidecar", lambda h: h.announcement_declarations_apply(),
     "meta/mob_announcements/declarations/apply", {}),
    ("declarations_get", lambda h: h.announcement_declarations_get(),
     "meta/mob_announcements/declarations/get", {}),
]


@pytest.mark.asyncio
@pytest.mark.parametrize("name,call,method,params", DECLARATION_SERVED, ids=[row[0] for row in DECLARATION_SERVED])
async def test_every_declaration_method_sends_its_wire_name_and_params(name, call, method, params):
    handle, calls = make_mock_mob_handle()
    await call(handle)
    assert calls == [(method, params)]


@pytest.mark.asyncio
async def test_a_role_template_put_reads_the_template():
    from meerkat_meta import AnnouncementRoleSelector
    handle, calls = make_mock_mob_handle({"meta/mob_announcements/subscriptions/put": {"subscription": TEMPLATE}})
    template = await handle.announcement_role_template_put(
        "op-t", "reviewers", AnnouncementRoleSelector(mob_id="mob-a", role="reviewer"), ("ops.alert",))
    assert calls[0][1]["members"] == ROLE
    assert (template.id, template.members.mob_id, template.members.role) == ("reviewers", "mob-a", "reviewer")
    assert template.topics == ("ops.alert",) and template.mobs == ("mob-a",)
    assert (template.state, template.revision, template.created_at_ms, template.updated_at_ms) == ("active", 1, 10, 10)


@pytest.mark.asyncio
async def test_a_template_verb_reads_the_template_and_its_children():
    child = {**SUBSCRIPTION, "id": "reviewers/0123456789abcdef", "state": "paused"}
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/subscriptions/pause": {
        "template": {**TEMPLATE, "state": "paused", "revision": 2}, "children": [child],
        "unresolved_batches": ["batch-1"]}})
    change = await handle.announcement_subscription_pause("op-p", "reviewers", 1)
    assert change.subscription is None
    assert (change.template.state, change.template.revision) == ("paused", 2)
    assert [(c.id, c.state) for c in change.children] == [("reviewers/0123456789abcdef", "paused")]
    assert change.unresolved_batches == ("batch-1",)


@pytest.mark.asyncio
async def test_a_subscription_verb_names_no_template():
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/subscriptions/cancel": {
        "subscription": {**SUBSCRIPTION, "state": "cancelled"}, "unresolved_batches": []}})
    change = await handle.announcement_subscription_cancel("op-c", "sub-1", 2)
    assert change.subscription.state == "cancelled"
    assert change.template is None and change.children == ()


@pytest.mark.asyncio
async def test_subscription_get_reads_a_role_template():
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/subscriptions/get": {
        "subscription": None, "marker": None, "template": TEMPLATE}})
    view = await handle.announcement_subscription_get("reviewers")
    assert view.subscription is None and view.marker is None
    assert (view.template.id, view.template.members.role) == ("reviewers", "reviewer")
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/subscriptions/get": {
        "subscription": SUBSCRIPTION, "marker": None}})
    assert (await handle.announcement_subscription_get("sub-1")).template is None


@pytest.mark.asyncio
async def test_declarations_read_their_records():
    handle, _ = make_mock_mob_handle({
        "meta/mob_announcements/declarations/get": DECLARATIONS_VIEW,
        "meta/mob_announcements/declarations/apply": {**DECLARATIONS_VIEW, "revision": 4}})
    view = await handle.announcement_declarations_get()
    assert (view.revision, view.digest) == (3, "sha256:abc")
    source, pending, child, future = view.records
    assert (source.kind, source.id, source.status, source.collection) == ("source", "mail", "applied", "live")
    assert (pending.status, pending.refusal_kind) == ("pending", "unknown_member")
    assert pending.refusal == {"kind": "unknown_member", "message": "no such member"}
    assert (child.kind, child.delivery_kind, child.delivery["cause"]) == ("role_child", "refused", "governed_runtime")
    assert (future.kind, future.status, future.refusal, future.collection, future.delivery) == (
        "a_future_kind", "a_future_status", None, None, None)
    assert (await handle.announcement_declarations_apply(DECLARATIONS)).revision == 4


@pytest.mark.parametrize("kind,data", [
    ("declaration_rollback", {"applied": 4}),
    ("declaration_digest_conflict", {"revision": 3}),
    ("role_template_limit", {"limit": 256}),
    ("subscription_id_taken", {"subscription_id": "reviewers"}),
])
def test_declaration_refusals_keep_the_refused_class(kind, data):
    from meerkat_meta import AnnouncementRefusedError
    error = _error(-32047, kind, **data)
    assert isinstance(error, AnnouncementRefusedError) and error.kind == kind
    assert all(error.data[key] == value for key, value in data.items())


def test_the_declaration_names_are_exported():
    import meerkat_meta
    for name in ("AnnouncementRoleSelector", "AnnouncementRoleTemplate", "AnnouncementDeclarationRecord",
                 "AnnouncementDeclarationsView"):
        assert name in meerkat_meta.__all__


# -- Handling reads and awaiting handling ------------------------------------

HANDLING_READ = {"pending": 1, "handled": 4, "handled_unverified": 2, "unhandled": 1, "held": 3,
                 "oldest_unhandled_at_ms": 70}


@pytest.mark.asyncio
async def test_subscription_put_sends_delivery_only_when_set():
    handle, calls = make_mock_mob_handle()
    await handle.announcement_subscription_put(
        "op-s", "sub-1", MEMBER, ["ops.alert"], delivery={"await_handling": True})
    assert calls[0][1]["delivery"] == {"await_handling": True}
    handle, calls = make_mock_mob_handle()
    await handle.announcement_subscription_put("op-s", "sub-1", MEMBER, ["ops.alert"])
    assert "delivery" not in calls[0][1]


@pytest.mark.asyncio
async def test_a_subscription_reads_its_delivery():
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/subscriptions/get": {
        "subscription": {**SUBSCRIPTION, "delivery": {"await_handling": False}}, "marker": None}})
    subscription = (await handle.announcement_subscription_get("sub-1")).subscription
    assert subscription.delivery == {"await_handling": False} and subscription.await_handling is False
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/subscriptions/get": {
        "subscription": SUBSCRIPTION, "marker": None}})
    subscription = (await handle.announcement_subscription_get("sub-1")).subscription
    assert subscription.delivery is None and subscription.await_handling is None


@pytest.mark.asyncio
async def test_member_get_reads_its_handling():
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/members/get": {**MEMBER_READ, "handling": HANDLING_READ}})
    handling = (await handle.announcement_member_get(MEMBER)).handling
    assert (handling.pending, handling.handled, handling.handled_unverified) == (1, 4, 2)
    assert (handling.unhandled, handling.held, handling.oldest_unhandled_at_ms) == (1, 3, 70)
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/members/get": {
        **MEMBER_READ, "handling": {**HANDLING_READ, "oldest_unhandled_at_ms": None}}})
    assert (await handle.announcement_member_get(MEMBER)).handling.oldest_unhandled_at_ms is None
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/members/get": MEMBER_READ})
    assert (await handle.announcement_member_get(MEMBER)).handling is None


def test_status_reads_the_handling_counts():
    from meerkat_meta import StatusResult
    announcements = {"sources": {"active": 1, "paused": 0, "failing": 0, "stale": 0},
                     "members": {"suspended": 0, "unhandled_held": 5},
                     "handling": {"pending": 2, "handled": 9, "handled_unverified": 1, "unhandled": 3},
                     "escalations": {"open": 1, "last_event_cursor": 4}}
    data = {"contract_version": "0.5.0", "running": True, "loaded_modules": [], "mob_announcements": announcements}
    counts = StatusResult.from_dict(data).mob_announcements
    assert counts.members["unhandled_held"] == 5
    assert counts.handling == {"pending": 2, "handled": 9, "handled_unverified": 1, "unhandled": 3}
    older = {key: value for key, value in announcements.items() if key != "handling"}
    assert StatusResult.from_dict({**data, "mob_announcements": older}).mob_announcements.handling == {}


@pytest.mark.parametrize("cause", ["unhandled", "handled_unverified"])
def test_a_handling_cause_reads_as_sent(cause):
    from meerkat_meta import AnnouncementEscalation
    escalation = AnnouncementEscalation.from_dict(
        {**ESCALATION, "cause": cause, "subject": {"kind": "member", "member": MEMBER}})
    assert (escalation.cause, escalation.subject_kind) == (cause, "member")


def test_the_member_handling_name_is_exported():
    import meerkat_meta
    assert "AnnouncementMemberHandling" in meerkat_meta.__all__


# -- Template deliveries -------------------------------------------------------

@pytest.mark.asyncio
async def test_role_template_put_sends_delivery_only_when_set():
    handle, calls = make_mock_mob_handle()
    await handle.announcement_role_template_put(
        "op-t", "reviewers", ROLE, ["ops.alert"], delivery={"await_handling": False})
    assert calls[0][1]["delivery"] == {"await_handling": False}
    handle, calls = make_mock_mob_handle()
    await handle.announcement_role_template_put("op-t", "reviewers", ROLE, ["ops.alert"])
    assert "delivery" not in calls[0][1]


@pytest.mark.asyncio
async def test_a_role_template_reads_its_delivery():
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/subscriptions/get": {
        "subscription": None, "marker": None, "template": {**TEMPLATE, "delivery": {"await_handling": True}}}})
    template = (await handle.announcement_subscription_get("reviewers")).template
    assert template.delivery == {"await_handling": True} and template.await_handling is True
    handle, _ = make_mock_mob_handle({"meta/mob_announcements/subscriptions/put": {"subscription": TEMPLATE}})
    template = await handle.announcement_role_template_put("op-t", "reviewers", ROLE, ["ops.alert"])
    assert template.delivery is None and template.await_handling is None
