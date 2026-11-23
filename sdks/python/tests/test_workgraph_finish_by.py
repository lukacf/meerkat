import pytest
from meerkat.generated.types import WorkItem
from meerkat.errors import MeerkatError


def test_generated_work_item_preserves_finish_by_and_legacy_activation():
    timestamp = "2026-11-18T08:00:00Z"
    raw = {
        "id": "finish-by-item", "realm_id": "test", "namespace": "default",
        "title": "Informational target", "status": "open", "priority": "medium",
        "completion_policy": {"kind": "self_attest"},
        "failed_child_join_policy": "require_success", "cancelled_child_join_policy": "require_success",
        "machine_state": {}, "revision": 1, "created_at": timestamp, "updated_at": timestamp,
    }
    target = "2030-01-02T03:04:05Z"
    parsed = WorkItem.from_wire({**raw, "due_at": timestamp, "finish_by": target})
    assert parsed.finish_by == target
    assert parsed.due_at == timestamp
    assert WorkItem.from_wire(raw).finish_by is None
    assert WorkItem.from_wire({**raw, "finish_by": None}).finish_by is None
    with pytest.raises(MeerkatError) as error:
        WorkItem.from_wire({**raw, "finish_by": 42})
    assert "finish_by" in str(error.value)
