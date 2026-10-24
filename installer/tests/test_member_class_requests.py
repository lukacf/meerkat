"""Forge mapping and call ordering; these tests cannot qualify native admission.

SDK cases require the selected real Toolkit artifact. Client spies check the
host boundary only; they cannot prove native authority or an admitted member.
"""
import asyncio
from copy import deepcopy
import json
from types import SimpleNamespace

import pytest

from launchkit.connect_installer_caller import CustodyError
from launchkit.connect_member_class_requests import (
    _installation_inputs, _roster_member, bind_member_class_requests, _member_class_add_request,
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
        self.value["owner"] = {"deployment": "fixture-deployment", "native_host": self.native}
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
    request = _member_class_add_request(f.callbacks, f.deployment, admission_id="saved-add-one",
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
    cold = _member_class_add_request(f.callbacks, f.deployment, admission_id="saved-add-one",
        class_id="reader", spec=sdk.Spec.from_dict(wire["spec"]))
    assert cold.wire() == wire
    another = _member_class_add_request(f.callbacks, f.deployment, admission_id="saved-add-two",
        class_id="reader", spec=sdk.Spec.from_dict(wire["spec"]))
    assert another.route["id"] != request.route["id"]
    assert another.controller["issuance_id"] != request.controller["issuance_id"]
    f.value["owner"]["deployment"] = "other-deployment"
    f.select()
    f.observe()
    other_deployment = _member_class_add_request(f.callbacks, f.deployment, admission_id="saved-add-one",
        class_id="reader", spec=sdk.Spec.from_dict(wire["spec"]))
    assert other_deployment.route["id"] != request.route["id"]
    assert other_deployment.controller["issuance_id"] != request.controller["issuance_id"]
    assert cold.wire() == wire
    assert _installation_inputs(f.callbacks)[0] == f.human


@pytest.mark.parametrize("case", ["missing-owner", "missing-deployment", "realm", "classes", "no-route", "ambiguous-domain",
    "requester", "subject", "ingress", "authentication", "event", "missing-namespace",
    "logical-owner", "human-domain", "audience", "duplicate-member", "unqualified-ingress"])
def test_missing_or_inconsistent_selected_mapping_is_refused(tmp_path, case):
    f = RequestFixture(tmp_path)
    route = f.native["routes"][0]
    if case == "missing-owner": del f.value["owner"]
    elif case == "missing-deployment": del f.value["owner"]["deployment"]
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
        _member_class_add_request(f.callbacks, f.deployment, **intent)


@pytest.mark.parametrize("change", ["runtime", "context-runtime", "application", "callbacks", "shutdown", "stop", "owner"])
async def test_bound_entry_refuses_stale_lifetime_before_mapping(tmp_path, monkeypatch, change):
    f = RequestFixture(tmp_path)
    result = object()
    runtime = SimpleNamespace(member_classes=SimpleNamespace(begin_add=lambda request, **kwargs: result))
    context = SimpleNamespace(runtime=runtime, stop_event=asyncio.Event())
    app = SimpleNamespace(dynamic=True, context=context, deployment=f.deployment, _application_callbacks=f.callbacks)
    owner = SimpleNamespace(runtime=runtime, application=app, callbacks=f.callbacks,
        _shutdown_task=None, _require_current=lambda: None)
    seen = []
    def prepare(callbacks, deployment, **intent):
        seen.append((callbacks, deployment, intent))
        return SimpleNamespace(wire=lambda: {"id": intent["admission_id"]})
    async def retain(value): pass
    async def publish(): pass
    monkeypatch.setattr("launchkit.connect_member_class_requests._member_class_add_request", prepare)
    bind_member_class_requests(owner, app, runtime)
    assert await context._begin_member_class_add(admission_id="one", retain_request=retain, publish=publish) is result
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
    with pytest.raises(CustodyError):
        await context._begin_member_class_add(admission_id="two", retain_request=retain, publish=publish)
    assert len(seen) == 1


async def test_context_has_no_mapping_before_connected_host(tmp_path, monkeypatch):
    host = load_host(monkeypatch, tmp_path)
    context = host.ApplicationContext(tmp_path, {}, asyncio.Event())
    with pytest.raises(RuntimeError, match="connected selected"):
        await context.begin_member_class_add(admission_id="one", class_id="reader", spec=object(),
            retain_request=lambda value: None, publish=lambda: None)
    assert not hasattr(context, "member_class_add_request")
    bind_member_class_requests(object(), SimpleNamespace(dynamic=False), object())


@pytest.mark.parametrize("timeout", [True, False, 0, -1, float("inf"), float("-inf"), float("nan"), "60", []])
async def test_invalid_timeout_refuses_before_retention_or_submission(tmp_path, sdk, timeout):
    f = RequestFixture(tmp_path)
    events = []
    runtime = SimpleNamespace(member_classes=SimpleNamespace(
        begin_add=lambda *args, **kwargs: events.append("native-submit")))
    context = SimpleNamespace(runtime=runtime, stop_event=asyncio.Event())
    app = SimpleNamespace(dynamic=True, context=context, deployment=f.deployment, _application_callbacks=f.callbacks)
    owner = SimpleNamespace(runtime=runtime, application=app, callbacks=f.callbacks,
        _shutdown_task=None, _require_current=f.callbacks._current)
    async def retain(value): events.append("app-retained")
    async def publish(): events.append("app-listed")
    bind_member_class_requests(owner, app, runtime)
    with pytest.raises(ValueError, match="timeout must be positive and finite"):
        await context._begin_member_class_add(admission_id="saved-add", class_id="reader",
            spec=sdk.Spec(identity="reader:one", profile="reusable-reader"),
            retain_request=retain, publish=publish, timeout=timeout)
    assert events == []


@pytest.mark.parametrize("failure", ["stop", "changed-plan", "invalidated", "retention-error", "cancelled"])
async def test_no_native_submission_when_retention_or_currentness_fails(tmp_path, monkeypatch, failure):
    f = RequestFixture(tmp_path)
    runtime = SimpleNamespace(member_classes=SimpleNamespace(
        begin_add=lambda *args, **kwargs: pytest.fail("No Add may be submitted")))
    context = SimpleNamespace(runtime=runtime, stop_event=asyncio.Event())
    app = SimpleNamespace(dynamic=True, context=context, deployment=f.deployment, _application_callbacks=f.callbacks)
    owner = SimpleNamespace(runtime=runtime, application=app, callbacks=f.callbacks,
        _shutdown_task=None, _require_current=lambda: None)
    def prepare(*args, **kwargs):
        return SimpleNamespace(wire=lambda: {"id": "original-request"})
    async def retain(value):
        assert isinstance(value, str)
        if failure == "stop": context.stop_event.set()
        elif failure == "changed-plan": f.path.write_bytes(f.path.read_bytes() + b" ")
        elif failure == "invalidated": f.callbacks._invalidated = True
        elif failure == "retention-error": raise OSError("durable write failed")
        else: raise asyncio.CancelledError()
    async def publish(): pytest.fail("No listing may occur")
    monkeypatch.setattr("launchkit.connect_member_class_requests._member_class_add_request", prepare)
    bind_member_class_requests(owner, app, runtime)
    error = {"retention-error": OSError, "cancelled": asyncio.CancelledError}.get(failure, CustodyError)
    with pytest.raises(error):
        await context._begin_member_class_add(admission_id="one", retain_request=retain, publish=publish)


@pytest.mark.parametrize("timeout", [60.0, 17.5, 1, None])
async def test_host_retains_immutable_original_then_submits_private_request_once(tmp_path, sdk, timeout):
    f = RequestFixture(tmp_path)
    spec = sdk.Spec(identity="reader:one", profile="reusable-reader", context={"records": ["original"]})
    events = []
    saved = tmp_path / "app-admitting-request.json"
    attempt = object()
    def begin(request, *, publish, timeout):
        assert isinstance(request, sdk.Request)
        assert request.wire() == json.loads(saved.read_text())
        assert request.spec.context == {"records": ["original"]}
        assert publish is publish_listed
        assert timeout == selected_timeout
        events.append("native-submit")
        # A native write has an exact correlation. Do not lose it by checking
        # currentness again after the synchronous SDK submission has happened.
        context.stop_event.set()
        return attempt
    runtime = SimpleNamespace(member_classes=SimpleNamespace(begin_add=begin))
    context = SimpleNamespace(runtime=runtime, stop_event=asyncio.Event())
    app = SimpleNamespace(dynamic=True, context=context, deployment=f.deployment, _application_callbacks=f.callbacks)
    owner = SimpleNamespace(runtime=runtime, application=app, callbacks=f.callbacks,
        _shutdown_task=None, _require_current=f.callbacks._current)
    async def retain(value):
        assert isinstance(value, str)
        saved.write_text(value)
        changed = json.loads(value)
        changed["route"]["represented_subject"]["id"] = "forged-subject"
        spec.context["records"].append("changed-after-retention")
        events.append("app-retained")
    async def publish_listed(): pytest.fail("The host cannot invoke listing itself")
    selected_timeout = timeout
    bind_member_class_requests(owner, app, runtime)
    result = await context._begin_member_class_add(admission_id="saved-add", class_id="reader", spec=spec,
        retain_request=retain, publish=publish_listed, **({"timeout": timeout} if timeout != 60.0 else {}))
    assert result is attempt
    assert events == ["app-retained", "native-submit"]


@pytest.mark.parametrize("extra", [{"mcp": object()}, {"mcp": None}, {"console_enrollment_id": 42}])
async def test_host_entry_refuses_mcp_and_enrollment_before_retention(tmp_path, extra):
    f = RequestFixture(tmp_path)
    runtime = SimpleNamespace(member_classes=SimpleNamespace(
        begin_add=lambda *args, **kwargs: pytest.fail("MCP entry must not submit")))
    context = SimpleNamespace(runtime=runtime, stop_event=asyncio.Event())
    app = SimpleNamespace(dynamic=True, context=context, deployment=f.deployment, _application_callbacks=f.callbacks)
    owner = SimpleNamespace(runtime=runtime, application=app, callbacks=f.callbacks,
        _shutdown_task=None, _require_current=f.callbacks._current)
    async def retain(value): pytest.fail("MCP entry must not retain")
    async def publish(): pytest.fail("MCP entry must not list")
    bind_member_class_requests(owner, app, runtime)
    with pytest.raises(TypeError, match="unexpected keyword argument"):
        await context._begin_member_class_add(admission_id="saved-add", class_id="reader", spec=object(),
            retain_request=retain, publish=publish, **extra)
