import pytest
from meerkat_meta.errors import ContractMismatchError
from meerkat_meta.types import WorkGraphItem
from .test_rpc_method_names import make_mock_mob_handle


def test_finish_by_return_model_preserves_new_and_missing_values():
    value = "2026-11-20T08:00:00Z"
    assert WorkGraphItem.from_dict({"id": "work_test", "finish_by": value}).finish_by == value
    assert WorkGraphItem.from_dict({"id": "work_test"}).finish_by is None


@pytest.mark.asyncio
async def test_finish_by_update_returns_value_and_sends_exact_operation():
    value = "2026-11-20T08:00:00Z"
    handle, calls = make_mock_mob_handle({"meta/workgraph/update": {"item": {"id": "work_test", "revision": 2, "finish_by": value}}})
    item = await handle.workgraph_update("work_test", 1, finish_by={"op": "set", "value": value})
    assert item.finish_by == value
    assert calls[-1] == ("meta/workgraph/update", {"id": "work_test", "expected_revision": 1, "finish_by": {"op": "set", "value": value}})
    handle, calls = make_mock_mob_handle({"meta/workgraph/update": {"item": {"id": "work_test", "revision": 3}}})
    item = await handle.workgraph_update("work_test", 2, finish_by={"op": "clear"})
    assert item.finish_by is None
    assert calls[-1][1]["finish_by"] == {"op": "clear"}


@pytest.mark.asyncio
@pytest.mark.parametrize("operation,returned", [
    ("create", None), ("goal_create", None), ("set", None),
    ("set", "2030-01-02T03:04:05.123456788Z"),
    ("clear", "2030-01-02T03:04:05.123456789Z"), ("clear", 42),
])
async def test_finish_by_helpers_raise_when_the_gateway_did_not_apply_it(operation, returned):
    value = "2030-01-02T03:04:05.123456789Z"
    method = {"create": "create", "goal_create": "goal/create"}.get(operation, "update")
    response = {"item": {"id": "work_test", "revision": 9, "finish_by": returned}, "attention": {}}
    handle, calls = make_mock_mob_handle({f"meta/workgraph/{method}": response})
    with pytest.raises(ContractMismatchError, match="finish_by"):
        if operation == "create":
            await handle.workgraph_create("Target", finish_by=value)
        elif operation == "goal_create":
            await handle.workgraph_goal_create("Target", {"kind": "session", "session_id": "session_1"}, finish_by=value)
        else:
            update = {"op": "clear"} if operation == "clear" else {"op": "set", "value": value}
            await handle.workgraph_update("work_test", 1, finish_by=update)
    assert len(calls) == 1  # A mismatch never repeats an already-committed mutation.


@pytest.mark.asyncio
async def test_finish_by_helpers_compare_only_the_target_and_preserve_timestamp_precision():
    value = "2030-01-02T04:04:05.123456789+01:00"
    echoed = "2030-01-02T03:04:05.123456789Z"
    response = {"item": {"id": "work_test", "revision": 99, "title": "Concurrent title", "finish_by": echoed}, "attention": {}}
    handle, calls = make_mock_mob_handle({method: response for method in ["meta/workgraph/create", "meta/workgraph/goal/create", "meta/workgraph/update"]})
    assert (await handle.workgraph_create("Target", finish_by=value)).finish_by == echoed
    assert (await handle.workgraph_goal_create("Target", {"kind": "session", "session_id": "session_1"}, finish_by=value)).item.finish_by == echoed
    assert (await handle.workgraph_update("work_test", 1, finish_by={"op": "set", "value": value})).finish_by == echoed
    assert len(calls) == 3
    handle, _ = make_mock_mob_handle({"meta/workgraph/update": {"item": {"id": "work_test", "finish_by": "2030-01-02T03:04:05.1200Z"}}})
    await handle.workgraph_update("work_test", 1, finish_by={"op": "set", "value": "2030-01-02T03:04:05.12+00:00"})


@pytest.mark.asyncio
async def test_finish_by_keep_does_not_assert_an_unrequested_target():
    response = {"item": {"id": "work_test", "finish_by": "2030-01-02T03:04:05Z"}}
    handle, calls = make_mock_mob_handle({"meta/workgraph/update": response})
    assert (await handle.workgraph_update("work_test", 1)).finish_by == response["item"]["finish_by"]
    assert (await handle.workgraph_update("work_test", 2, finish_by=None)).finish_by == response["item"]["finish_by"]
    assert len(calls) == 2


@pytest.mark.asyncio
@pytest.mark.parametrize("requested,returned", [
    ("2030- 1-2 3:4:5.1200000009 UTC", "2030-01-02T03:04:05.120Z"),
    ("0000-01-02T01:02:03.4+01:00", "0000-01-02T00:02:03.400Z"),
    ("0000-01-01T00:30:00+01:00", "-0001-12-31T23:30:00Z"),
    ("2000-03-01T00:30:00+01:00", "2000-02-29T23:30:00Z"),
    ("1900-03-01T00:30:00+01:00", "1900-02-28T23:30:00Z"),
    ("+10000-01-02T01:02:03.1234567899+0100", "+10000-01-02T00:02:03.123456789Z"),
    ("2016-12-31T23:59:60Z", "2016-12-31T23:59:60+00:00"),
])
async def test_finish_by_helper_accepts_chrono_normalized_response(requested, returned):
    handle, calls = make_mock_mob_handle({"meta/workgraph/update": {"item": {"id": "work_test", "finish_by": returned}}})
    assert (await handle.workgraph_update("work_test", 1, finish_by={"op": "set", "value": requested})).finish_by == returned
    assert len(calls) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("operation", ["create", "goal_create", "set", "clear", "malformed", "unrecognized_timestamp"])
async def test_finish_by_mismatch_preserves_committed_item_for_read_recovery(operation):
    value = "2030-01-02T03:04:05Z"
    method = {"create": "create", "goal_create": "goal/create", "unrecognized_timestamp": "create"}.get(operation, "update")
    returned = {"id": "work_returned", "revision": 9, "finish_by": value if operation in ("clear", "unrecognized_timestamp") else None}
    handle, calls = make_mock_mob_handle({
        f"meta/workgraph/{method}": {"item": returned, "attention": {}},
        "meta/workgraph/get": {"item": returned},
    })
    with pytest.raises(ContractMismatchError, match="may already have committed") as caught:
        if operation in ("create", "unrecognized_timestamp"):
            requested = value if operation == "create" else "server-accepted-spelling-outside-comparator"
            await handle.workgraph_create("Target", finish_by=requested)
        elif operation == "goal_create":
            await handle.workgraph_goal_create("Target", {"kind": "session", "session_id": "session_1"}, finish_by=value)
        else:
            update = {"op": "set", "value": value} if operation == "set" else {"op": "clear"}
            if operation == "malformed":
                update["value"] = None  # An old gateway might accept an unknown/malformed field.
            await handle.workgraph_update("work_requested", 1, finish_by=update)
    error = caught.value
    assert error.returned_item_id == "work_returned"
    assert error.returned_item_revision == 9
    assert len(calls) == 1  # No automatic retry of the mutation.
    recovered = await handle.workgraph_get(error.returned_item_id)
    assert recovered.id == error.returned_item_id
    assert recovered.revision == error.returned_item_revision
    assert [name for name, _ in calls] == [f"meta/workgraph/{method}", "meta/workgraph/get"]
    assert calls[-1][1] == {"id": "work_returned"}


@pytest.mark.asyncio
@pytest.mark.parametrize("item", [
    None, [], {},
    {"id": 42, "revision": 9}, {"id": "", "revision": 9}, {"id": " ", "revision": 9},
    *({"id": "work_returned", "revision": revision} for revision in [None, "9", True, 1.5, -1, 2**64]),
])
async def test_finish_by_mismatch_does_not_invent_recovery_identity(item):
    handle, calls = make_mock_mob_handle({"meta/workgraph/create": {"item": item}})
    with pytest.raises(ContractMismatchError) as caught:
        await handle.workgraph_create("Target", finish_by="2030-01-02T03:04:05Z")
    assert caught.value.returned_item_id is None
    assert caught.value.returned_item_revision is None
    assert len(calls) == 1
