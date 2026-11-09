"""Application intent boundaries without a native owner or permission grant."""
from copy import deepcopy
import hashlib
import json

import pytest

from launchkit.connect_policy_input import (
    MAX_POLICY_SELECTION_BYTES,
    application_tool_policy_input,
    application_tool_policy_selection,
)


def selection():
    return {"provider_id": "application", "policy_id": "observing-tools", "revision": 1,
        "members": [
            {"member_identity": "observer:main", "grants": [
                {"tool_name": "forecast", "action": "invoke", "consequence": "r0"},
                {"tool_name": "schedule", "action": "invoke", "consequence": "r2"}]},
            {"member_identity": "reviewer:main", "grants": []},
        ]}


def encoded(value):
    return (json.dumps(value, ensure_ascii=True, separators=(",", ":")) + "\n").encode()


def read(raw, members=("observer:main", "reviewer:main")):
    return application_tool_policy_selection(raw, selected_member_identities=members)


def test_source_preserves_explicit_order_empty_grants_and_classification_without_authority():
    source = selection()
    source["members"].reverse()
    source["members"][1]["grants"].reverse()
    raw = encoded(source)
    value = read(raw)
    assert value == source
    assert value["members"][0]["grants"] == []
    assert set(value) == {"provider_id", "policy_id", "revision", "members"}
    assert not any(name in value for name in ("default_deny", "source", "policy_digest", "read_only"))
    value["members"][1]["grants"].clear()
    assert read(raw) == source


@pytest.mark.parametrize("change", ["missing_member", "foreign_member", "duplicate_member",
    "duplicate_tool", "implicit_grants", "unknown_field", "missing_classification",
    "category_grant", "Boolean_revision", "zero_revision", "overflow_revision",
    "invalid_action", "action_object", "classification_bool", "classification_list",
    "classification_uppercase", "classification_unknown", "spaced_identity",
    "spaced_tool", "empty_provider", "lone_surrogate"])
def test_incomplete_ambiguous_or_non_native_source_is_refused(change):
    value = deepcopy(selection())
    member = value["members"][0]
    grant = member["grants"][0]
    if change == "missing_member":
        value["members"].pop()
    elif change == "foreign_member":
        member["member_identity"] = "another:main"
    elif change == "duplicate_member":
        value["members"].append(deepcopy(member))
    elif change == "duplicate_tool":
        member["grants"].append(deepcopy(grant))
    elif change == "implicit_grants":
        del member["grants"]
    elif change == "unknown_field":
        value["default_deny"] = False
    elif change == "missing_classification":
        del grant["consequence"]
    elif change == "category_grant":
        member["grants"] = {"category": "research"}
    elif change in {"Boolean_revision", "zero_revision", "overflow_revision"}:
        value["revision"] = {"Boolean_revision": True, "zero_revision": 0,
                             "overflow_revision": 2**64}[change]
    elif change in {"invalid_action", "action_object"}:
        grant["action"] = "approve" if change == "invalid_action" else {"invoke": True}
    elif change.startswith("classification_"):
        grant["consequence"] = {"classification_bool": False, "classification_list": ["r0"],
            "classification_uppercase": "R0", "classification_unknown": "r4"}[change]
    elif change == "spaced_identity":
        member["member_identity"] += " "
    elif change == "spaced_tool":
        grant["tool_name"] = " forecast"
    elif change == "empty_provider":
        value["provider_id"] = ""
    else:
        grant["tool_name"] = "\ud800"
    with pytest.raises(ValueError):
        read(encoded(value))


@pytest.mark.parametrize("raw", [b"", b"{}\n{}\n", b"\xff", b"[", b"null",
    b'{"provider_id":"one","provider_id":"two"}',
    b'{"revision":NaN}', b'{"revision":Infinity}', b"[" * 2000 + b"]" * 2000])
def test_bad_document_is_a_controlled_refusal(raw):
    with pytest.raises(ValueError):
        read(raw)


def test_oversize_source_refuses_before_json_parsing(monkeypatch):
    def unexpected(*args, **kwargs):
        raise AssertionError("Oversize bytes must not enter JSON parsing.")
    monkeypatch.setattr(json, "loads", unexpected)
    with pytest.raises(ValueError, match="16 MiB"):
        read(b" " * (MAX_POLICY_SELECTION_BYTES + 1))


@pytest.mark.parametrize("members", ["observer:main", b"observer:main", None,
    {"observer:main": True}, ["observer:main", "observer:main"], [False]])
def test_inventory_cannot_be_a_string_mapping_boolean_or_duplicate(members):
    with pytest.raises((TypeError, ValueError)):
        read(encoded(selection()), members)


def test_native_input_keeps_exact_original_provenance_and_requires_native_member_rows():
    # These strings are opaque specimens, not an asserted native ID algorithm.
    source = selection()
    source["members"][0]["member_identity"] = "opaque-native-observer"
    source["members"][1]["member_identity"] = "opaque-native-reviewer"
    raw = b"  " + encoded(source)
    value = json.loads(application_tool_policy_input(raw,
        source_id="accepted-package/tool-policy.json",
        selected_member_identities=("opaque-native-observer", "opaque-native-reviewer")))
    assert value == {**source, "source": {"source_id": "accepted-package/tool-policy.json",
        "source_digest": "sha256:" + hashlib.sha256(raw).hexdigest()}}
    with pytest.raises(ValueError, match="inventory"):
        application_tool_policy_input(encoded(selection()),
            source_id="accepted-package/tool-policy.json",
            selected_member_identities=("opaque-native-observer", "opaque-native-reviewer"))


def test_large_population_and_utf8_names_remain_exact_without_category_expansion():
    source = selection()
    identities = tuple(f"astronomer:{index}" for index in range(1, 635))
    source["revision"] = 2**64 - 1
    source["members"] = [{"member_identity": identity, "grants": [
        {"tool_name": "mcp_observatory_find_Å_target", "action": "invoke", "consequence": "r1"},
        {"tool_name": "setup_report", "action": "invoke", "consequence": "r3"}]}
        for identity in reversed(identities)]
    assert read(encoded(source), identities) == source
