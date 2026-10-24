"""Observed requirement adapter with the selected SDK, not native grant proof."""
import copy
from dataclasses import asdict
import json
import sys
from types import SimpleNamespace

import pytest

from test_member_mcp_application import application, sdk


def observed(sdk):
    from meerkat_toolkit.host_control import PrivateSourceEvidence
    selection = sdk.MemberMcpAdmissionSelection("original-add", "project-one", "project")
    source = PrivateSourceEvidence("requirements/project-one", "revision-7", "a" * 64)
    lifetime = {"bound": {"bound": "window", "window": {"not_before_ms": 10, "expires_at_ms": 20}},
                "unresolved": []}
    return sdk.ObservedMemberMcpRequirement(selection, source, (
        sdk.MemberMcpServiceIntent("zeta", ("read",), lifetime),
        sdk.MemberMcpServiceIntent("alpha", ("inspect",), copy.deepcopy(lifetime))))


@pytest.mark.parametrize("missing", ["retain_member_mcp", "lookup_member_mcp", "admission_roster"])
def test_observation_requires_existing_admission_store_hooks(tmp_path, monkeypatch, missing):
    app = application(tmp_path, monkeypatch)
    app.integration.retain_member_mcp = lambda value: None
    app.integration.lookup_member_mcp = lambda key: None
    app.integration.observe_member_mcp_requirement = lambda key, source: None
    delattr(app.integration, missing)
    with pytest.raises(ValueError):
        app.member_mcp_runtime_options()


def test_ordinary_participant_does_not_import_or_pass_observed_sdk_api(tmp_path, monkeypatch):
    app = application(tmp_path, monkeypatch)
    app.integration.retain_member_mcp = lambda value: None
    app.integration.lookup_member_mcp = lambda key: None
    calls = []
    def legacy(retain, lookup):
        calls.append((retain, lookup))
        return object()
    monkeypatch.setitem(sys.modules, "meerkat_toolkit.member_classes", SimpleNamespace(
        MemberMcpApplication=legacy, MemberMcpAdmissionSelection=object, RetainedMemberMcpBindings=object))
    assert app.member_mcp_runtime_options()["member_mcp"] is app._member_mcp
    assert len(calls) == 1


async def test_observation_retains_full_same_row_association_for_cold_cleanup(tmp_path, monkeypatch, sdk):
    requirement = observed(sdk)
    path = tmp_path / "application-row.json"
    path.write_text(json.dumps({"state": "admitting", "requirement": requirement.wire()}))
    calls = []

    def bind(app):
        def observe(key, source):
            row = json.loads(path.read_text())
            calls.append((key, source))
            if row["state"] != "admitting" or row["requirement"]["selection"] != key:
                return None
            return row["requirement"]
        def retain(value):
            row = json.loads(path.read_text())
            if row["state"] != "admitting" or value["accepted_requirement"] != row["requirement"]:
                raise ValueError("Actual application requirement changed")
            if "retained" in row and row["retained"] != value:
                raise ValueError("Original admission binding changed")
            row["retained"] = value
            path.write_text(json.dumps(row))
            # A callback's private JSON is detached from the native request.
            value["accepted_requirement"]["services"][0]["lifetime"]["bound"]["window"]["expires_at_ms"] = 99
        def lookup(key):
            row = json.loads(path.read_text())
            if row["retained"]["selection"] != key:
                raise ValueError("Another admission")
            return row["retained"]
        app.integration.observe_member_mcp_requirement = observe
        app.integration.retain_member_mcp = retain
        app.integration.lookup_member_mcp = lookup

    app = application(tmp_path, monkeypatch)
    bind(app)
    participant = app.member_mcp_runtime_options()["member_mcp"]
    assert participant.requirement_contract == "observed_requirement"
    actual = await participant._observed_requirement(requirement.selection, requirement.source)
    assert actual == requirement
    assert calls == [(requirement.selection.wire(), asdict(requirement.source))]
    value = sdk.RetainedMemberMcpBindings(requirement.selection, ("pending-zeta", "pending-alpha"), actual)
    await participant._retain(value)
    assert actual.wire() == requirement.wire()
    await app.stop()

    row = json.loads(path.read_text())
    row["state"] = "void"
    path.write_text(json.dumps(row))
    cold = application(tmp_path, monkeypatch)
    bind(cold)
    restored = cold.member_mcp_runtime_options()["member_mcp"]
    assert await restored._observed_requirement(requirement.selection, requirement.source) is None
    assert await restored._lookup(requirement.selection) == value
    with pytest.raises(ValueError, match="changed"):
        await restored._retain(value)
    assert json.loads(path.read_text())["state"] == "void"
    await cold.stop()


@pytest.mark.parametrize("change", ["admission", "revision", "hash", "contract"])
async def test_observation_refuses_foreign_source_or_selection(tmp_path, monkeypatch, sdk, change):
    requirement = observed(sdk)
    wire = requirement.wire()
    if change == "admission":
        wire["selection"]["admission_id"] = "successor-add"
    elif change == "revision":
        wire["source"]["revision"] = "revision-8"
    elif change == "hash":
        wire["source"]["sha256"] = "b" * 64
    else:
        wire["contract"] = "current_owner"
    app = application(tmp_path, monkeypatch)
    app.integration.retain_member_mcp = lambda value: pytest.fail("Observation must not retain")
    app.integration.lookup_member_mcp = lambda key: pytest.fail("Observation must not look up bindings")
    app.integration.observe_member_mcp_requirement = lambda key, source: wire
    participant = app.member_mcp_runtime_options()["member_mcp"]
    with pytest.raises((ValueError, RuntimeError)):
        await participant._observed_requirement(requirement.selection, requirement.source)
    await app.stop()


async def test_observation_preserves_empty_services_and_detaches_returned_data(tmp_path, monkeypatch, sdk):
    requirement = observed(sdk)
    wire = requirement.wire()
    app = application(tmp_path, monkeypatch)
    app.integration.retain_member_mcp = lambda value: None
    app.integration.lookup_member_mcp = lambda key: None
    app.integration.observe_member_mcp_requirement = lambda key, source: wire
    participant = app.member_mcp_runtime_options()["member_mcp"]
    snapshot = await participant._observed_requirement(requirement.selection, requirement.source)
    wire["services"][0]["lifetime"]["bound"]["window"]["expires_at_ms"] = 99
    assert snapshot.wire() == requirement.wire()
    wire["services"] = []
    empty = await participant._observed_requirement(requirement.selection, requirement.source)
    assert empty.services == ()
    await app.stop()
