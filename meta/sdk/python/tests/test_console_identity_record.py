"""ConsoleIdentityRecord: the typed identity -> (session, profile) map.

This record is the supported replacement for hosts reading the continuity
store directly (a downstream app's positional profile recovery). The parsing
contract mirrors the Rust `ConsoleIdentityRecord` serde shape:
`session_id` is optional (skip-serialized when absent), `topology_peers`
and `labels` default to empty.
"""

from meerkat_meta import ConsoleIdentityRecord


def test_full_record_round_trips_the_wire_shape() -> None:
    record = ConsoleIdentityRecord.from_dict(
        {
            "identity": "agent:alpha",
            "display_name": "Alpha",
            "runtime_key": "rt-key",
            "runtime_member_id": "mk--alpha",
            "session_id": "0198aaaa-0000-7000-8000-000000000001",
            "visibility": "visible",
            "addressable": True,
            "health": "ok",
            "topology_peers": ["agent:beta"],
            "labels": {"role": "default", "agent_identity": "agent:alpha"},
        }
    )
    assert record.identity == "agent:alpha"
    assert record.session_id == "0198aaaa-0000-7000-8000-000000000001"
    # The adopted roster profile rides labels in the shipped gateways.
    assert record.labels["role"] == "default"
    assert record.topology_peers == ["agent:beta"]
    assert record.addressable is True


def test_optional_fields_default_like_the_serde_skips() -> None:
    record = ConsoleIdentityRecord.from_dict(
        {
            "identity": "agent:beta",
            "display_name": "",
            "runtime_key": "",
            "runtime_member_id": "",
            "visibility": "visible",
            "addressable": False,
            "health": "ok",
        }
    )
    assert record.session_id is None
    assert record.topology_peers == []
    assert record.labels == {}
