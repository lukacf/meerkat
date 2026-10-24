"""Forge input mapping only; these tests cannot qualify native admission.

SDK cases require the selected real Toolkit artifact. No fake native reply or
Console enrollment is used to turn a prepared request into an admitted member.
"""
import asyncio
from copy import deepcopy
import json
from types import SimpleNamespace

import pytest

from launchkit.connect_installer_caller import CustodyError
from launchkit.connect_member_class_requests import (
    _installation_inputs, _roster_member, bind_member_class_requests, member_class_add_request,
)
from test_application_extension import load_host
from test_selected_member_classes import ClassFixture


class RequestFixture(ClassFixture):
    def __init__(self, root):
        super().__init__(root)
        # Deliberately different human and installation domains: neither may be
        # replaced by a connector subject or copied into the other's role.
        self.human = {"kind": "human", "id": "selected-operator", "qualification": {
            "kind": "qualified", "trust_domain_id": "oidc-issuer:fixture"}}
        self.ingress = {"kind": "runtime_host", "id": "console-ingress", "qualification": {
            "kind": "qualified", "trust_domain_id": "meta-forge-installation:fixture"}}
        binding = deepcopy(self.members[1]["member"])
        logical = {"kind": "shared_agent", "id": "reader:main",
            "qualification": deepcopy(self.ingress["qualification"])}
        self.native = {"realm": "fixture-realm", "remote_operator": self.human,
            "ingress_domain": {"authority": self.ingress, "namespace": "selected-ingress"},
            "member_classes": deepcopy(self.value["application"]["member_classes"]),
            "members": [{"member": binding, "logical_owner": logical, "context_generation": 1,
                "audience": {"audience": "principal", "principal": deepcopy(self.human)}}],
            "routes": [{"id": "selected-seed-route", "origin": {"kind": "local_console",
                "member": deepcopy(binding), "requester": deepcopy(self.human),
                "ingress_actor": deepcopy(self.ingress)}, "logical_executor": deepcopy(logical),
                "represented_subject": deepcopy(self.human), "source_authority": deepcopy(self.ingress),
                "authentication_domain": {"authority": deepcopy(self.ingress), "namespace": "selected-auth"},
                "event_domain": {"authority": deepcopy(self.ingress), "namespace": "selected-events"},
                "application_parent": None, "controller_parent": None}]}
        self.value["application"]["realm"] = {"realm_id": "fixture-realm"}
        self.value["owner"] = {"native_host": self.native}
        self.select()
        self.observe()


@pytest.fixture
def sdk():
    classes = pytest.importorskip("meerkat_toolkit.member_classes")
    models = pytest.importorskip("meerkat_meta.identity_first_models")
    return SimpleNamespace(Request=classes.MemberClassAddRequest, Spec=models.DurableAgentSpec)


# Fixed vectors from Meta's public member_comms_id codec, including its marker
# collision, underscore escaping and Unicode scalar (not UTF-8 byte) encoding.
@pytest.mark.parametrize("identity,member", [
    ("reader", "reader"), ("_reader-2", "_reader-2"), ("9reader", "mk--9reader"),
    ("mk--reader", "mk--mk--reader"), ("reader:one", "mk--reader_cone"),
    ("reader:_a", "mk--reader_c__a"), ("team:\u00e5", "mk--team_c_xe5_"),
    ("team:\U0001f642", "mk--team_c_x1f642_"),
    ("reader.one", "mk--reader_x2e_one"),
])
def test_public_member_codec(identity, member):
    assert _roster_member(identity) == member


def test_request_copies_selected_facts_and_survives_cold_reconstruction(tmp_path, sdk):
    f = RequestFixture(tmp_path)
    spec = sdk.Spec(identity="reader:one", profile="reusable-reader", context={"records": ["one"]})
    request = member_class_add_request(f.callbacks, f.deployment, admission_id="saved-add-one",
        class_id="reader", spec=spec)
    assert isinstance(request, sdk.Request)
    wire = json.loads(json.dumps(request.wire()))
    member = {"mob_id": "selected-app", "member": "mk--reader_cone", "role": "reusable-reader"}
    assert wire["instance"] == {"class": "reader", "member": {
        "member": member, "context_generation": 1,
        "logical_owner": {"kind": "shared_agent", "id": "reader:one", "qualification": f.ingress["qualification"]},
        "audience": {"audience": "principal", "principal": f.human}}}
    route = wire["route"]
    assert route["origin"] == {"kind": "local_console", "member": member,
        "requester": f.human, "ingress_actor": f.ingress}
    assert route["logical_executor"] == wire["instance"]["member"]["logical_owner"]
    assert route["represented_subject"] == f.human
    assert route["source_authority"] == f.ingress
    assert route["authentication_domain"] == f.native["routes"][0]["authentication_domain"]
    assert route["event_domain"] == f.native["routes"][0]["event_domain"]
    assert "application_parent" not in route and "controller_parent" not in route
    assert wire["controller"]["target"] == {"kind": "original_route", "route": route["id"]}
    assert "controller_route" not in wire["controller"]
    assert "mcp" not in wire and "expected_predecessor" not in wire
    # Application mutation cannot change the retained request.
    spec.context["records"].append("later")
    assert request.wire() == wire
    f.select()
    f.observe()
    cold = member_class_add_request(f.callbacks, f.deployment, admission_id="saved-add-one",
        class_id="reader", spec=sdk.Spec.from_dict(wire["spec"]))
    assert cold.wire() == wire
    another = member_class_add_request(f.callbacks, f.deployment, admission_id="saved-add-two",
        class_id="reader", spec=sdk.Spec.from_dict(wire["spec"]))
    assert another.route["id"] != request.route["id"]
    assert another.controller["issuance_id"] != request.controller["issuance_id"]
    request.route["origin"]["requester"]["id"] = "changed-output"
    assert cold.wire() == wire
    assert _installation_inputs(f.callbacks)[0] == f.human


@pytest.mark.parametrize("case", ["missing-owner", "realm", "classes", "no-route", "ambiguous-domain",
    "requester", "subject", "ingress", "authentication", "event", "missing-namespace",
    "logical-owner", "human-domain", "audience", "duplicate-member", "unqualified-ingress"])
def test_missing_or_inconsistent_selected_mapping_is_refused(tmp_path, case):
    f = RequestFixture(tmp_path)
    route = f.native["routes"][0]
    if case == "missing-owner": del f.value["owner"]
    elif case == "realm": f.native["realm"] = "another-realm"
    elif case == "classes": f.native["member_classes"] = []
    elif case == "no-route": f.native["routes"] = []
    elif case == "ambiguous-domain":
        second = deepcopy(route)
        second["authentication_domain"]["namespace"] = "another-auth"
        f.native["routes"].append(second)
    elif case == "requester": route["origin"]["requester"]["id"] = "connector-user"
    elif case == "subject": route["represented_subject"]["id"] = "connector-user"
    elif case == "ingress": route["origin"]["ingress_actor"]["id"] = "another-host"
    elif case == "authentication": route["authentication_domain"]["authority"] = deepcopy(f.human)
    elif case == "event": route["event_domain"]["authority"] = deepcopy(f.human)
    elif case == "missing-namespace": del route["event_domain"]["namespace"]
    elif case == "logical-owner": f.native["members"][0]["logical_owner"]["id"] = "another-owner"
    elif case == "human-domain":
        route["logical_executor"]["qualification"] = deepcopy(f.human["qualification"])
        f.native["members"][0]["logical_owner"] = deepcopy(route["logical_executor"])
    elif case == "audience": f.native["members"][0]["audience"] = {"audience": "public"}
    elif case == "duplicate-member": f.native["members"].append(deepcopy(f.native["members"][0]))
    else: f.native["ingress_domain"]["authority"]["qualification"] = {"kind": "unqualified"}
    f.select()
    with pytest.raises(CustodyError):
        _installation_inputs(f.callbacks)


@pytest.mark.parametrize("case", ["unobserved", "changed-plan", "unknown-class", "wrong-profile",
    "seed-identity", "placement", "initial-message", "not-spec", "mutated-spec", "empty-id",
    "large-id", "invalid-deadline", "principal-override"])
def test_request_refuses_invalid_intent_or_lost_selection(tmp_path, sdk, case):
    f = RequestFixture(tmp_path)
    intent = {"admission_id": "saved-add", "class_id": "reader",
        "spec": sdk.Spec(identity="reader:one", profile="reusable-reader")}
    if case == "unobserved": f.select()
    elif case == "changed-plan": f.path.write_bytes(f.path.read_bytes() + b" ")
    elif case == "unknown-class": intent["class_id"] = "foreign"
    elif case == "wrong-profile": intent["spec"].profile = "reader-source"
    elif case == "seed-identity": intent["spec"].identity = "reader:main"
    elif case == "placement": intent["spec"].placement = "remote-host"
    elif case == "initial-message": intent["spec"].initial_message = "Start before held completion"
    elif case == "not-spec": intent["spec"] = intent["spec"].to_dict()
    elif case == "mutated-spec": intent["spec"].identity = "invalid identity"
    elif case == "empty-id": intent["admission_id"] = " "
    elif case == "large-id": intent["admission_id"] = "a" * 257
    elif case == "invalid-deadline": intent["hold_timeout_ms"] = 0
    else: intent["requester"] = f.human
    with pytest.raises((CustodyError, ValueError, TypeError, RuntimeError)):
        member_class_add_request(f.callbacks, f.deployment, **intent)


@pytest.mark.parametrize("change", ["runtime", "context-runtime", "application", "callbacks", "shutdown", "stop", "owner"])
def test_bound_factory_refuses_stale_lifetime_before_mapping(tmp_path, monkeypatch, change):
    f = RequestFixture(tmp_path)
    runtime = object()
    context = SimpleNamespace(runtime=runtime, stop_event=asyncio.Event())
    app = SimpleNamespace(dynamic=True, context=context, deployment=f.deployment, _application_callbacks=f.callbacks)
    owner = SimpleNamespace(runtime=runtime, application=app, callbacks=f.callbacks,
        _shutdown_task=None, _require_current=lambda: None)
    seen = []
    result = object()
    def prepare(callbacks, deployment, **intent):
        seen.append((callbacks, deployment, intent))
        return result
    monkeypatch.setattr("launchkit.connect_member_class_requests.member_class_add_request", prepare)
    bind_member_class_requests(owner, app, runtime)
    assert context._member_class_requests(admission_id="one") is result
    assert seen == [(f.callbacks, f.deployment, {"admission_id": "one"})]
    if change == "runtime": owner.runtime = object()
    elif change == "context-runtime": context.runtime = object()
    elif change == "application": owner.application = object()
    elif change == "callbacks": app._application_callbacks = object()
    elif change == "shutdown": owner._shutdown_task = object()
    elif change == "stop": context.stop_event.set()
    else:
        def expired(): raise CustodyError("Expired owner")
        owner._require_current = expired
    with pytest.raises(CustodyError): context._member_class_requests(admission_id="two")
    assert len(seen) == 1


def test_context_has_no_mapping_before_connected_host(tmp_path, monkeypatch):
    host = load_host(monkeypatch, tmp_path)
    context = host.ApplicationContext(tmp_path, {}, asyncio.Event())
    with pytest.raises(RuntimeError, match="connected selected"):
        context.member_class_add_request(admission_id="one", class_id="reader", spec=object())
    bind_member_class_requests(object(), SimpleNamespace(dynamic=False), object())


def test_bound_factory_rechecks_owner_after_preparation(tmp_path, monkeypatch):
    f = RequestFixture(tmp_path)
    runtime = object()
    context = SimpleNamespace(runtime=runtime, stop_event=asyncio.Event())
    app = SimpleNamespace(dynamic=True, context=context, deployment=f.deployment, _application_callbacks=f.callbacks)
    owner = SimpleNamespace(runtime=runtime, application=app, callbacks=f.callbacks,
        _shutdown_task=None, _require_current=lambda: None)
    def prepare(*args, **kwargs):
        context.stop_event.set()
        return object()
    monkeypatch.setattr("launchkit.connect_member_class_requests.member_class_add_request", prepare)
    bind_member_class_requests(owner, app, runtime)
    with pytest.raises(CustodyError): context._member_class_requests(admission_id="one")
