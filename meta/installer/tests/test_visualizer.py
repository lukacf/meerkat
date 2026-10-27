import asyncio
import copy
import hashlib
import json
from types import SimpleNamespace

import pytest

from launchkit.blueprint import Blueprint, starter_blueprint
from launchkit.projects import fingerprint
from launchkit.visualizer import (
    COMPOSITION_GUIDANCE, CONTRACT_VERSION, DYNAMIC_CLASS_GUIDANCE, LIFECYCLE_GUIDANCE, RESPONSIVE_POPULATION_GUIDANCE, VISUALIZER_PROMPT, VisualizerBusy, VisualizerPending, VisualizerService, VisualizerUnavailable, checked_feedback, checked_inputs, checked_revision_context, inspection_graph, revision_context_binding, visualization_context, visualization_examples,
)


CODE = "return {duration:36,render(frame){frame.ctx.clearRect(0,0,frame.width,frame.height);},hitTest(x,y){return model.nodes[0].id;}};"


def inputs():
    spec = starter_blueprint()
    model = {"nodes": [{"id": identity, "name": identity, "kind": "agent"} for identity in spec.identities()],
             "edges": [{"id": "peer:one", "source": "lead:main", "target": "research:1", "kind": "peer"}],
             "phases": [{"id": "request", "activeNodeIds": ["lead:main"], "activeEdgeIds": ["peer:one"]}]}
    return spec.model_dump(mode="json"), model


def large_inspection_inputs():
    spec = starter_blueprint().model_dump(mode="json")
    research = next(item for item in spec["agent_classes"] if item["id"] == "research")
    research["instances"] = 80
    research["members"] = [{"key": f"member-{i:03}", "name": f"Context owner {i:03}",
                            "instructions": f"Own case {i:03} and its distinct evidence. Consult only explicitly assigned peers."}
                           for i in range(80)]
    peers = [{"id": f"peer:{i}", "source": "lead:main" if i < 40 else "review:main",
              "target": f"research:member-{i:03}", "kind": "peer", "directed": i >= 40,
              "label": f"Display label {i}"} for i in range(80)]
    spec["topology"] = {"mode": "manual", "entrypoint": "lead:main",
                        "edges": [{"a": edge["source"], "b": edge["target"]} for edge in peers]}
    spec["flows"] = [{"id": "review-case", "name": "Review one case", "steps": [
        {"id": "examine", "agent": "research:member-000", "prompt": "Examine this case."},
        {"id": "review", "agent": "review:main", "prompt": "Review the case evidence.", "depends_on": ["examine"]}]}]
    blueprint = Blueprint.model_validate(spec)
    nodes = [{"id": identity, "name": member.name if item.members else item.name,
              "kind": "agent", "classId": item.id,
              "instructions": item.instructions + ("\n" + member.instructions if item.members else "")}
             for item in blueprint.agent_classes
             for identity, member in zip(item.identities(), item.members or [None] * item.instances)]
    nodes.extend({"id": identity, "name": name, "kind": kind} for identity, name, kind in (
        ("human", "One human", "input"), ("files", "Evidence files", "workspace"),
        ("archive", "Archived evidence", "workspace"), ("notes", "Shared notes", "notes"),
        ("dispatch", "Explicit native dispatch", "workflow"), ("result", "Returned result", "output")))
    tools = []
    for i in range(80):
        identity = f"research:member-{i:03}"
        for target in ("files", "archive", "notes"):
            tools.append({"id": f"tool:{i}:{target}", "source": identity, "target": target,
                          "kind": "tool", "capability": "notes" if target == "notes" else "workspace",
                          "access": "write" if target == "notes" else "read",
                          "toolNames": ["read_text"] if target != "notes" else
                                       ["update_case"] if i < 20 else ["read_case"],
                          "custom_contract": {"revision": 1}})
    boundaries = [
        {"id": "human-contact", "source": "human", "target": "lead:main", "kind": "input"},
        {"id": "native-dispatch", "source": "dispatch", "target": "research:member-000",
         "kind": "activation", "targetStepId": "examine"},
        {"id": "native-dependency", "source": "research:member-000", "target": "review:main",
         "kind": "flow", "sourceStepId": "examine", "targetStepId": "review"},
        {"id": "returned-result", "source": "review:main", "target": "result", "kind": "output"}]
    model = {"nodes": nodes, "edges": peers + tools + boundaries,
             "phases": [{"id": "examine", "title": "Examine this case", "description": "An exact proposed handoff.",
                         "activeNodeIds": ["research:member-000", "review:main"],
                         "activeEdgeIds": ["peer:0", "tool:0:notes", "native-dependency"]}],
             "flows": [{"id": "review-case", "name": "Review one case", "steps": ["examine", "review"]}],
             "entrypoint": "lead:main", "selectedFlowId": "review-case"}
    return checked_inputs(blueprint.model_dump(mode="json"), model)


def inspected_job(service, data=None, *, key="current-job"):
    blueprint, model = data or large_inspection_inputs()
    service.pending = {"key": key, "blueprint": blueprint, "model": model, "contract": CONTRACT_VERSION,
                       "blueprint_hash": fingerprint(blueprint), "model_hash": fingerprint(model)}
    return service.pending


class NativeMembers:
    """Existing typed SDK member operations, without a gateway or provider."""
    def __init__(self):
        self.members, self.sessions = {}, {}
        self.ensures, self.retirements = [], []
        self.session_override = None

    async def ensure_member(self, identity, role, **options):
        from meerkat_meta.types import MemberSnapshot
        self.ensures.append((identity, role, copy.deepcopy(options)))
        if identity not in self.members:
            self.members[identity] = MemberSnapshot(identity, role, "active", [], options["labels"])
            self.sessions[identity] = options.get("resume_session_id") or self.session_override or "session-" + identity
        return self.members[identity]

    async def member_status(self, identity):
        from meerkat_meta.types import RichMemberSnapshot
        return RichMemberSnapshot("idle", None, None, 0, False, self.sessions[identity])

    async def retire_member(self, identity):
        self.retirements.append(identity)
        self.members.pop(identity, None)


def native_admission(service, interaction="native", cursor=1):
    job = service.pending
    return {"cursor": cursor, "interaction_id": interaction,
            "identity": job.get("author_identity", "visualizer:main"),
            "session_id": job.get("author_session_id", "legacy-native-session")}


@pytest.fixture
def service(tmp_path):
    async def start():
        architect.starts += 1
    members = NativeMembers()
    architect = SimpleNamespace(root=tmp_path, runtime=SimpleNamespace(rust_http_base_url="http://127.0.0.1:1", mob_handle=lambda: members),
                                start=start, starts=0, context={"owner": "guide"}, proposal={"state": "unchanged"},
                                guide_context={"thread": "unchanged"}, message_lock=asyncio.Lock())
    result = VisualizerService(architect, timeout=3, poll_interval=0.001)
    result.native_members = members
    return result


def native_completion(service, monkeypatch, *, release=None, publish=True):
    calls = []
    async def request(client, method, url, **kwargs):
        calls.append((method, url, copy.deepcopy(kwargs)))
        if method == "post":
            assert kwargs["json"]["identity"] == service.pending.get("author_identity", "visualizer:main")
            return SimpleNamespace(json=lambda: native_admission(service, "native-visualization"))
        if release is not None:
            await release.wait()
        if publish and service.pending["contract"] == CONTRACT_VERSION:
            await service.inspect({"view": "overview", "request_key": service.pending["key"], "include_author_references": True})
            result = await service.publish({"request_key": service.pending["key"], "code": CODE, "summary": "A layered proposal."})
            assert result["valid"]
        return SimpleNamespace(json=lambda: {"frames": [
            {"kind": "interaction_complete", "interaction_id": "other-turn", "cursor": 2},
            {"kind": "interaction_complete", "interaction_id": "native-visualization", "cursor": 3}]})
    monkeypatch.setattr("launchkit.architect._native_request", request)
    return calls


def test_reference_tool_contains_both_actual_detailed_scene_factories():
    examples = visualization_examples()
    assert set(examples) == {"household", "organisation", "renderer"}
    assert "class Net" in examples["renderer"]
    assert "function household(W, H, S" in examples["household"]
    assert "PERSON AGENTS" in examples["household"]
    assert "function organisation(W, H, S" in examples["organisation"]
    assert "const NI = 100" in examples["organisation"]
    assert "function product(" not in examples["organisation"]
    assert "function householdAgent(" not in examples["household"]


def test_fresh_native_profile_uses_the_same_freeform_program_guidance_as_resumed_requests():
    # Persistent native profiles must not steer fresh sessions back to a layout
    # schema while the scoped request overrides obsolete resumed profiles.
    assert VISUALIZER_PROMPT.endswith(COMPOSITION_GUIDANCE)
    assert RESPONSIVE_POPULATION_GUIDANCE in VISUALIZER_PROMPT
    assert "distinct non-identity illustrative marks" in VISUALIZER_PROMPT
    assert "illustrative intra-cohort\nmessage flow" in VISUALIZER_PROMPT
    assert "complete labels and hit regions inside" in VISUALIZER_PROMPT
    assert "not over role labels" in VISUALIZER_PROMPT
    assert "freeform-animation-v3" in VISUALIZER_PROMPT
    assert "Canvas2D" in VISUALIZER_PROMPT and "OffscreenCanvas" in VISUALIZER_PROMPT
    assert "return a live program with duration and\nrender(frame)" in VISUALIZER_PROMPT
    assert "read_visualization_examples" in VISUALIZER_PROMPT
    assert "complete frozen factual context and both original images" in VISUALIZER_PROMPT
    assert "JavaScript examples are optional references" in VISUALIZER_PROMPT
    assert "counted population" in VISUALIZER_PROMPT
    assert "phases are explanatory context" in VISUALIZER_PROMPT
    assert "For a selected workflow (model.selectedFlowId)" in VISUALIZER_PROMPT
    assert "reduced-motion static view" in VISUALIZER_PROMPT
    assert LIFECYCLE_GUIDANCE in VISUALIZER_PROMPT
    assert "Preserve durable\nconfigured permission policies" in LIFECYCLE_GUIDANCE
    assert "wizard authorization" in LIFECYCLE_GUIDANCE
    assert "credential/authentication state" in LIFECYCLE_GUIDANCE
    assert "drawing text, captions,\nchapters or publication summaries" in LIFECYCLE_GUIDANCE
    assert "source prose or previous_program contain them" in LIFECYCLE_GUIDANCE
    for obsolete in ("The body returns JSON data", "positions every model.nodes ID",
                     "only an edge active in the named phase", "no DOM, network, module loader, canvas"):
        assert obsolete not in VISUALIZER_PROMPT


@pytest.mark.asyncio
async def test_source_backed_class_lifecycle_guidance_reaches_author_and_reviewer(service):
    from launchkit.visualization_review import REVIEW_PROMPT
    from test_profile_only_classes import graph, profile_plan

    blueprint = profile_plan()
    inspected_job(service, checked_inputs(blueprint, graph(blueprint)))
    overview = await service.inspect({"view": "overview"})
    for prompt in (VISUALIZER_PROMPT, overview["composition_guidance"]):
        assert DYNAMIC_CLASS_GUIDANCE in prompt
        assert "Zero initial members alone does not prove automatic discovery" in prompt
        assert "same continuing\ncontext when reuse is intended" in prompt
        assert "temporary helper that returns its result and leaves" in prompt
        assert "Keep illustrative future marks out of model.nodes" in prompt
    assert "unless the source describes them" in overview["profile_only_guidance"]
    assert "illustrative and non-selectable" in overview["profile_only_guidance"]
    review = " ".join(REVIEW_PROMPT.split())
    assert "zero-start class definitions are not initial agents" in review
    assert "Source-backed future members can be illustrated as distinct non-selectable marks" in review
    assert "later work visibly returns to the same continuing context" in review
    assert "Do not demand a new member, helper, retirement or schedule that the source does not describe" in review


def view_scope_inputs(research_count, selected_flow):
    blueprint, model = inputs()
    next(item for item in blueprint["agent_classes"] if item["id"] == "research")["instances"] = research_count
    blueprint = Blueprint.model_validate(blueprint).model_dump(mode="json")
    model["nodes"] = [{"id": identity, "name": identity, "kind": "agent"}
                      for identity in Blueprint.model_validate(blueprint).identities()]
    model["nodes"].extend([{"id": "human", "name": "Person", "kind": "input"},
                           {"id": "notes", "name": "Shared notes", "kind": "notes"}])
    model["edges"].extend([
        {"id": "contact", "source": "human", "target": "lead:main", "kind": "input"},
        {"id": "lead-notes", "source": "lead:main", "target": "notes", "kind": "tool", "access": "write"},
    ])
    model["selectedFlowId"] = selected_flow
    model["flows"] = copy.deepcopy(blueprint["flows"])
    return checked_inputs(blueprint, model)


@pytest.mark.asyncio
@pytest.mark.parametrize("has_brief", [False, True], ids=["owner-instructions", "current-brief"])
async def test_intended_human_interaction_author_context_preserves_configured_source(service, has_brief):
    blueprint, model = inputs()
    instructions = "Own the person's ongoing conversation, consult the researchers, and present their findings."
    # The conversation owner is deliberately different from the native default.
    next(item for item in blueprint["agent_classes"] if item["id"] == "review")["instructions"] = instructions
    next(node for node in model["nodes"] if node["id"] == "review:main")["instructions"] = instructions
    model["nodes"].extend([{"id": "human", "name": "Person", "kind": "input"},
                           {"id": "result", "name": "Answer", "kind": "output"}])
    if has_brief:
        model["design_context"] = {"facts": {"ownership": {
            "summary": "The review owner handles the person's conversation and answer.",
            "status": "confirmed", "source_message_ids": ["owner-choice"]}}, "requirements": []}
    blueprint, model = checked_inputs(blueprint, model)
    before = copy.deepcopy((blueprint, model))
    binding = service.request_binding(blueprint, model)
    key = binding["native_request_key"]
    service.pending = {**binding, "key": key, "blueprint": blueprint, "model": model}
    overview = await service.inspect({"request_key": key})
    detail = await service.inspect({"view": "full", "request_key": key})
    assert overview["request_key"] == key and detail["available"] is True
    guidance = overview["composition_guidance"]
    assert "Use the current brief and\nowner instructions" in guidance
    assert "even when model.edges has no human contact edge" in guidance
    assert "exact configured relationship or\na source-backed intended interaction" in guidance
    assert "Establish that distinction once where useful" in guidance
    assert "Preserve meaningful permission and approval boundaries" in guidance
    assert "derive configured relationships from runtime model.edges" in overview["projection"]
    if has_brief:
        assert "does not make explicitly intended ownership unresolved" in overview["design_context_guidance"]
        assert overview["model"]["design_context"] == model["design_context"]
    else:
        assert "design_context_guidance" not in overview
    assert detail["blueprint"] == blueprint and detail["model"] == model
    assert detail["model"]["edges"] == before[1]["edges"]
    assert not any(edge["kind"] in {"input", "output"} for edge in detail["model"]["edges"])
    assert service.request_binding(blueprint, model) == binding
    assert (blueprint, model) == before and service.architect.starts == 0


@pytest.mark.asyncio
@pytest.mark.parametrize("research_count", [2, 503], ids=["small-team", "hundreds-of-identities"])
@pytest.mark.parametrize("selected_flow", [None, "briefing"], ids=["whole-team", "selected-job"])
@pytest.mark.parametrize("has_prior_program", [False, True], ids=["fresh", "resumed-source"])
async def test_scoped_view_guidance_reaches_fresh_and_resumed_native_requests_without_rebinding_source(
        service, monkeypatch, research_count, selected_flow, has_prior_program):
    blueprint, model = view_scope_inputs(research_count, selected_flow)
    previous = None
    if has_prior_program:
        prior_blueprint = copy.deepcopy(blueprint)
        prior_blueprint["project"]["description"] = "Previous composition of the same configured team."
        prior_binding = service.request_binding(prior_blueprint, model)
        # A prior program may have made the exact sample-only mistake. Its code
        # stays intact as advisory context, while this request names its own view.
        prior_code = 'api.caption("One case sample", "A bounded example, not the whole team.");' + CODE
        previous = {**prior_binding, "key": prior_binding["native_request_key"], "code": prior_code,
                    "code_hash": hashlib.sha256(prior_code.encode()).hexdigest(), "summary": "One case sample."}
    context = revision_context(previous, setup="view-scope", stream=f"view-scope:{selected_flow or '__overview__'}")
    frozen = copy.deepcopy((blueprint, model, context))
    expected = service.request_binding(blueprint, model, revision_context=context)
    sends = []

    async def admission(*args):
        pass

    async def request(client, method, url, **kwargs):
        if method == "post":
            sends.append(kwargs["json"])
            inspected = await service.inspect({"view": "overview"})
            guidance = inspected["view_guidance"]
            assert "view_guidance" in kwargs["json"]["content"]
            assert guidance not in kwargs["json"]["content"]
            # Non-agent interfaces and stores must not inflate the source count.
            assert f"{research_count + 2} configured agent identities" in guidance
            if selected_flow:
                assert "Selected simulation view: workflow 'briefing' (Research briefing)" in guidance
                assert "does not need to draw the whole team" in guidance
                assert "full proposed team" not in guidance
            else:
                assert "Selected simulation view: Team & responsibilities" in guidance
                assert "3 agent classes" in guidance
                assert "not a sample case or the first workflow" in guidance
                assert "every configured identity its own compact mark" in guidance
                assert "model.selectedFlowId" in guidance
            assert RESPONSIVE_POPULATION_GUIDANCE not in kwargs["json"]["content"]
            assert RESPONSIVE_POPULATION_GUIDANCE in inspected["composition_guidance"]
            assert "label hierarchy" in inspected["composition_guidance"]
            assert "frame.selectedId" in inspected["composition_guidance"]
            assert "never silently show" in inspected["composition_guidance"]
            assert "Draw selected tool access from that identity's exact model.edges" in inspected["composition_guidance"]
            assert "subset of its exact grant relationships before" in inspected["composition_guidance"]
            assert "draw that subset once" in inspected["composition_guidance"]
            assert "shown and total grant counts" in inspected["composition_guidance"]
            assert "read/write, capability" in inspected["composition_guidance"]
            assert "no prescribed line limit or resource layout" in inspected["composition_guidance"]
            assert "no required number of" in inspected["composition_guidance"]
            assert "not a required grid, ring, class schema or agent quota" in inspected["composition_guidance"]
            assert "positions" not in inspected["output_contract"]
            assert inspected["model"]["selectedFlowId"] == selected_flow
            assert inspected["blueprint"]["agent_classes"] == blueprint["agent_classes"]
            assert [node["id"] for node in inspected["model"]["nodes"]] == [node["id"] for node in model["nodes"]]
            full = await service.inspect({"view": "full", "request_key": inspected["request_key"]})
            assert full["blueprint"] == blueprint and full["model"] == model
            assert inspected["previous_program"] == context["previous_program"]
            await service.inspect({"view": "overview", "request_key": service.pending["key"], "include_author_references": True})
            # Authoring guidance does not impose a new layout/population gate on
            # publication. This test is not evidence of generated visual quality.
            assert (await service.publish({"request_key": inspected["request_key"], "code": CODE,
                                           "summary": "A freely authored program."}))["valid"]
            return SimpleNamespace(json=lambda: native_admission(service, "scoped-view", 1))
        return SimpleNamespace(json=lambda: {"frames": [{"kind": "interaction_complete", "cursor": 2,
                                                         "interaction_id": "scoped-view"}]})

    monkeypatch.setattr("launchkit.architect._native_request", request)
    artifact = await service.generate_continuous_with_native_admission(blueprint, model, revision_context=context,
        on_admission=admission, on_native_admission=admission)
    assert artifact["key"] == expected["native_request_key"] and artifact["code"] == CODE
    assert artifact["blueprint_hash"] == fingerprint(blueprint) and artifact["model_hash"] == fingerprint(model)
    assert artifact["publication_receipt"]["revision_binding"] == expected["revision_binding"]
    assert (blueprint, model, context) == frozen and len(sends) == 1


@pytest.mark.asyncio
async def test_legacy_absent_selection_keeps_full_team_scope_and_exact_late_identity(service):
    blueprint, model = view_scope_inputs(503, None)
    model.pop("selectedFlowId")
    job = inspected_job(service, (blueprint, model))
    before = copy.deepcopy(job)
    result = await service.inspect({"view": "overview"})
    assert "Selected simulation view: Team & responsibilities" in result["view_guidance"]
    assert "505 configured agent identities" in result["view_guidance"]
    assert "selectedFlowId" not in result["model"]
    assert "research:503" in {node["id"] for node in result["model"]["nodes"]}
    detail = await service.inspect({"view": "node", "request_key": job["key"], "id": "research:503"})
    assert detail["node"] == next(node for node in model["nodes"] if node["id"] == "research:503")
    assert job == before


@pytest.mark.asyncio
async def test_view_guidance_names_the_selected_job_instead_of_the_first_blueprint_flow(service):
    blueprint, model = view_scope_inputs(503, "second-job")
    second = copy.deepcopy(blueprint["flows"][0])
    second.update(id="second-job", name="Independent evidence assessment")
    blueprint["flows"].append(second)
    blueprint = Blueprint.model_validate(blueprint).model_dump(mode="json")
    model["flows"] = copy.deepcopy(blueprint["flows"])
    job = inspected_job(service, checked_inputs(blueprint, model))
    before = copy.deepcopy(job)
    result = await service.inspect({"view": "overview"})
    assert "workflow 'second-job' (Independent evidence assessment)" in result["view_guidance"]
    assert "(Research briefing)" not in result["view_guidance"]
    assert "does not need to draw the whole team" in result["view_guidance"]
    assert result["model"]["selectedFlowId"] == "second-job"
    assert job == before



@pytest.mark.asyncio
@pytest.mark.parametrize("has_prior_program", [False, True], ids=["fresh", "resumed-source"])
async def test_requested_worker_capacity_remains_source_not_configured_identities(
        service, monkeypatch, has_prior_program):
    blueprint, model = inputs()
    capacity = ("Requested capacity for an 800-temporary-worker burst. Elastic spawning, "
                "worker identities and lifecycle remain unimplemented application work; "
                "the configured durable identities are seed owners, not this worker roster. "
                "The planned cohort gathers evidence and exchanges findings for owner review; "
                "its spawning and permission implementation remain unresolved.")
    blueprint["project"]["description"] = capacity
    model["selectedFlowId"] = None
    blueprint, model = checked_inputs(blueprint, model)
    previous = None
    if has_prior_program:
        prior_blueprint = copy.deepcopy(blueprint)
        prior_blueprint["project"]["description"] = "Earlier requested capacity for 600 temporary workers."
        prior_binding = service.request_binding(prior_blueprint, model)
        prior_code = 'api.caption("Earlier capacity", "600 planned workers");' + CODE
        previous = {**prior_binding, "key": prior_binding["native_request_key"], "code": prior_code,
                    "code_hash": hashlib.sha256(prior_code.encode()).hexdigest(),
                    "summary": "Earlier proposed worker capacity."}
    context = revision_context(previous)
    frozen = copy.deepcopy((blueprint, model, context))
    configured_ids = set(Blueprint.model_validate(blueprint).identities())
    expected = service.request_binding(blueprint, model, revision_context=context)
    sends = []

    async def admission(*args):
        pass

    async def request(client, method, url, **kwargs):
        if method == "post":
            sends.append(kwargs["json"])
            inspected = await service.inspect({"view": "overview"})
            assert "composition_guidance" in kwargs["json"]["content"]
            for guidance in (VISUALIZER_PROMPT, inspected["composition_guidance"]):
                assert RESPONSIVE_POPULATION_GUIDANCE in guidance
                assert "distinct non-identity illustrative marks" in guidance
                assert "illustrative intra-cohort\nmessage flow" in guidance
                assert "not configured peer edges, access grants or" in guidance
                assert "keep those marks out of model.nodes" in guidance
                assert "complete labels and hit regions inside" in guidance
                assert "not over role labels" in guidance
            assert f"{len(configured_ids)} configured agent identities" in inspected["view_guidance"]
            assert "800 configured agent identities" not in inspected["view_guidance"]
            assert inspected["blueprint"]["project"]["description"] == capacity
            assert inspected["previous_program"] == context["previous_program"]
            full = await service.inspect({"view": "full", "request_key": inspected["request_key"]})
            assert full["blueprint"] == blueprint and full["model"] == model
            assert {node["id"] for node in full["model"]["nodes"] if node["kind"] == "agent"} == configured_ids
            assert full["model"]["edges"] == frozen[1]["edges"]
            await service.inspect({"view": "overview", "request_key": service.pending["key"], "include_author_references": True})
            # Source/count/request continuity is checked here. Rendering quality,
            # worker execution and native capacity need their own real evidence.
            assert (await service.publish({"request_key": inspected["request_key"], "code": CODE,
                                           "summary": "Configured owners and proposed capacity."}))["valid"]
            return SimpleNamespace(json=lambda: native_admission(service, "planned-capacity", 1))
        return SimpleNamespace(json=lambda: {"frames": [{"kind": "interaction_complete", "cursor": 2,
                                                         "interaction_id": "planned-capacity"}]})

    monkeypatch.setattr("launchkit.architect._native_request", request)
    artifact = await service.generate_continuous_with_native_admission(blueprint, model, revision_context=context,
        on_admission=admission, on_native_admission=admission)
    assert artifact["key"] == expected["native_request_key"]
    assert artifact["blueprint_hash"] == fingerprint(blueprint) and artifact["model_hash"] == fingerprint(model)
    assert artifact["publication_receipt"]["revision_binding"] == expected["revision_binding"]
    assert (blueprint, model, context) == frozen and len(sends) == 1

@pytest.mark.asyncio
@pytest.mark.parametrize("has_prior_program", [False, True], ids=["fresh", "resumed-source"])
async def test_scoped_lifecycle_guidance_preserves_durable_permissions_and_raw_context(service, monkeypatch, has_prior_program):
    blueprint, model = inputs()
    permissions = ("Only coordinators contact users. Reviewers may read case evidence. "
                   "Sending a message requires human approval.")
    interview_state = "The wizard has not admitted a build or authenticated the connector."
    blueprint["project"]["description"] = "Coordinate shared casework. " + interview_state
    lead = next(item for item in blueprint["agent_classes"] if item["id"] == "lead")
    lead["instructions"] += "\n" + permissions
    next(node for node in model["nodes"] if node["id"] == "lead:main")["instructions"] = permissions
    blueprint, model = checked_inputs(blueprint, model)
    previous = None
    if has_prior_program:
        prior_blueprint = copy.deepcopy(blueprint)
        prior_blueprint["project"]["description"] = "An earlier casework proposal. " + interview_state
        prior_binding = service.request_binding(prior_blueprint, model)
        prior_code = 'api.caption("Earlier wizard state", ' + json.dumps(interview_state) + ');' + CODE
        previous = {**prior_binding, "key": prior_binding["native_request_key"], "code": prior_code,
                    "code_hash": hashlib.sha256(prior_code.encode()).hexdigest(), "summary": interview_state}
    context = revision_context(previous)
    frozen = copy.deepcopy((blueprint, model, context))
    expected = service.request_binding(blueprint, model, revision_context=context)
    sends = []

    async def admission(*args):
        pass

    async def request(client, method, url, **kwargs):
        if method == "post":
            sends.append(kwargs["json"])
            prompt = kwargs["json"]["content"]
            # Current inspection supplies the boundary even when persistent
            # native profile instructions were created by an older host.
            assert "composition_guidance" in prompt
            assert LIFECYCLE_GUIDANCE not in prompt
            overview = await service.inspect({})
            assert LIFECYCLE_GUIDANCE in overview["composition_guidance"]
            assert overview["blueprint"]["project"]["description"] == blueprint["project"]["description"]
            inspected_lead = next(item for item in overview["blueprint"]["agent_classes"] if item["id"] == "lead")
            assert inspected_lead["instructions"] == lead["instructions"]
            detail = await service.inspect({"view": "full", "request_key": overview["request_key"]})
            assert detail["blueprint"] == blueprint and detail["model"] == model
            assert detail["model"]["nodes"][0]["instructions"] == permissions
            assert overview["previous_program"] == context["previous_program"]
            if has_prior_program:
                assert overview["previous_program"]["code"] == prior_code
                assert overview["previous_program"]["summary"] == interview_state
                assert "base_code_hash" in prompt and "edits:[{find,replace}]" in prompt
            await service.inspect({"view": "overview", "request_key": service.pending["key"], "include_author_references": True})
            assert (await service.publish({"request_key": overview["request_key"], "code": CODE,
                                           "summary": "Roles and configured approval boundaries."}))["valid"]
            return SimpleNamespace(json=lambda: native_admission(service, "scoped-lifecycle", 1))
        return SimpleNamespace(json=lambda: {"frames": [{"kind": "interaction_complete", "cursor": 2,
                                                         "interaction_id": "scoped-lifecycle"}]})

    monkeypatch.setattr("launchkit.architect._native_request", request)
    artifact = await service.generate_continuous_with_native_admission(blueprint, model, revision_context=context,
        on_admission=admission, on_native_admission=admission)
    assert artifact["key"] == expected["native_request_key"] and artifact["code"] == CODE
    assert artifact["blueprint_hash"] == fingerprint(blueprint) and artifact["model_hash"] == fingerprint(model)
    assert artifact["publication_receipt"]["revision_binding"] == expected["revision_binding"]
    assert (blueprint, model, context) == frozen and len(sends) == 1

    async def no_send(*args, **kwargs):
        raise AssertionError("Lifecycle guidance must not invalidate the saved same-design publication.")

    monkeypatch.setattr("launchkit.architect._native_request", no_send)
    cached = await service.generate_continuous_with_native_admission(blueprint, model, revision_context=context,
        on_admission=admission, on_native_admission=admission)
    assert cached["cached"] and cached["key"] == artifact["key"] and cached["code"] == CODE


@pytest.mark.asyncio
async def test_native_visualization_is_separate_from_guide_and_cache_is_private(service, monkeypatch):
    calls = native_completion(service, monkeypatch)
    before = copy.deepcopy((service.architect.context, service.architect.proposal, service.architect.guide_context))
    async with service.architect.message_lock:
        artifact = await service.generate(*inputs())
    assert artifact["source"] == "agent" and artifact["code"] == CODE and not artifact["cached"]
    assert not service.status()["pending"]
    assert (service.architect.context, service.architect.proposal, service.architect.guide_context) == before
    cached = await service.generate(*inputs())
    assert cached["key"] == artifact["key"] and cached["cached"]
    assert len(calls) == 2 and service.architect.starts == 1
    assert service.root.stat().st_mode & 0o077 == 0
    assert (service.root / f"{artifact['key']}.json").stat().st_mode & 0o077 == 0
    assert not (service.root / "pending.json").exists()


@pytest.mark.asyncio
async def test_each_changed_request_uses_a_fresh_native_author_with_checked_prior_source(service, monkeypatch):
    calls = native_completion(service, monkeypatch)
    original = await service.generate(*inputs())
    context = revision_context(original)
    frozen = copy.deepcopy(context)
    async def admitted(*_):
        pass
    revised = await service.generate_continuous_with_native_admission(*inputs(), feedback="Clarify the returned reply.",
        revision_context=context, on_admission=admitted, on_native_admission=admitted)
    assert original["author_identity"] != revised["author_identity"]
    assert original["author_session_id"] != revised["author_session_id"]
    assert revised["native_admission"]["identity"] == revised["author_identity"]
    assert revised["native_admission"]["session_id"] == revised["author_session_id"]
    assert revised["publication_receipt"]["revision_binding"] == revision_context_binding(frozen)
    assert context == frozen
    assert len(service.native_members.ensures) == 2
    for identity, role, options in service.native_members.ensures:
        assert role == "visualizer" and "resume_session_id" not in options and "context" not in options
        assert options["additional_instructions"] == [VISUALIZER_PROMPT]
        assert options["labels"]["visualization_author_identity"] == identity
    assert service.native_members.retirements == [original["author_identity"], revised["author_identity"]]
    assert len([call for call in calls if call[0] == "post"]) == 2
    assert (await service.generate_continuous_with_native_admission(*inputs(), feedback="Clarify the returned reply.",
        revision_context=frozen, on_admission=admitted, on_native_admission=admitted))["cached"]
    assert len(service.native_members.ensures) == 2


@pytest.mark.asyncio
async def test_pre_admission_timeout_reuses_saved_author_session_and_idempotency_after_restart(service, monkeypatch):
    complete = native_completion(service, monkeypatch)
    from launchkit.architect import _native_request
    attempts = []
    async def ambiguous(client, method, url, **kwargs):
        if method == "post":
            attempts.append(copy.deepcopy(kwargs["json"]))
            if len(attempts) == 1:
                raise TimeoutError()
        return await _native_request(client, method, url, **kwargs)
    monkeypatch.setattr("launchkit.architect._native_request", ambiguous)
    with pytest.raises(VisualizerPending):
        await service.generate(*inputs())
    saved = copy.deepcopy(service.pending)
    restored = VisualizerService(service.architect, timeout=3, poll_interval=.001)
    # Completion uses the restored service's pending tool context.
    resumed = native_completion(restored, monkeypatch)
    artifact = await restored.generate(*inputs())
    assert artifact["author_identity"] == saved["author_identity"]
    assert artifact["author_session_id"] == saved["author_session_id"]
    assert service.native_members.ensures[-1][2]["resume_session_id"] == saved["author_session_id"]
    assert resumed[0][2]["json"] == attempts[0]
    assert complete == [] and len(resumed) == 2


@pytest.mark.asyncio
async def test_pre_admission_restored_author_refuses_replacement_session_without_send(service, monkeypatch):
    async def unavailable(*_args, **_kwargs):
        raise TimeoutError()
    monkeypatch.setattr("launchkit.architect._native_request", unavailable)
    with pytest.raises(VisualizerPending):
        await service.generate(*inputs())
    saved = copy.deepcopy(service.pending)
    service.native_members.sessions[saved["author_identity"]] = "replacement-session"
    restored = VisualizerService(service.architect, timeout=3, poll_interval=.001)
    async def forbidden_send(*_args, **_kwargs):
        raise AssertionError("A changed native session cannot acquire this saved turn.")
    monkeypatch.setattr("launchkit.architect._native_request", forbidden_send)
    with pytest.raises(VisualizerUnavailable, match="does not match"):
        await restored.generate(*inputs())
    assert restored.pending == saved
    assert service.native_members.ensures[-1][2]["resume_session_id"] == saved["author_session_id"]
    assert service.native_members.retirements == []


@pytest.mark.asyncio
@pytest.mark.parametrize("wrong", ["identity", "session_id"])
async def test_fresh_author_refuses_foreign_native_admission_without_replacing_saved_owner(service, monkeypatch, wrong):
    sends = []
    async def request(client, method, url, **kwargs):
        assert method == "post"
        sends.append(kwargs["json"])
        admission = native_admission(service)
        admission[wrong] = "different-native-owner"
        return SimpleNamespace(json=lambda: admission)
    monkeypatch.setattr("launchkit.architect._native_request", request)
    with pytest.raises(ValueError, match="does not belong"):
        await service.generate(*inputs())
    saved = copy.deepcopy(service.pending)
    assert saved["admission"] is None and saved["author_session_id"]
    assert len(sends) == 1 and service.native_members.retirements == []
    native_completion(service, monkeypatch)
    artifact = await service.generate(*inputs())
    assert artifact["author_identity"] == saved["author_identity"]
    assert artifact["author_session_id"] == saved["author_session_id"]


@pytest.mark.asyncio
async def test_retirement_lost_ack_retries_terminal_owner_without_ensure_or_paid_turn(service, monkeypatch):
    calls = native_completion(service, monkeypatch)
    retire = service.native_members.retire_member
    attempts = []
    async def lost_ack(identity):
        attempts.append(identity)
        await retire(identity)
        if len(attempts) == 1:
            raise RuntimeError("Retirement acknowledgement unavailable")
    monkeypatch.setattr(service.native_members, "retire_member", lost_ack)
    with pytest.raises(RuntimeError, match="acknowledgement"):
        await service.generate(*inputs())
    saved = copy.deepcopy(service.pending)
    assert saved["completed"] and saved["author_retirement_started"] and not saved.get("author_retired")
    restored = VisualizerService(service.architect, timeout=3, poll_interval=.001)
    artifact = await restored.generate(*inputs())
    assert attempts == [saved["author_identity"]] * 2
    assert len(service.native_members.ensures) == 1 and len(calls) == 2
    assert artifact["native_admission"] == saved["admission"]
    assert restored.pending is None


@pytest.mark.asyncio
async def test_same_key_cached_publication_drains_later_terminal_retirement_debt(service, monkeypatch):
    calls = []
    published = False

    async def request(client, method, url, **kwargs):
        nonlocal published
        calls.append((method, copy.deepcopy(kwargs)))
        if method == "post":
            return SimpleNamespace(json=lambda: native_admission(service, "cached-then-terminal"))
        if not published:
            await service.inspect({"view": "overview", "request_key": service.pending["key"], "include_author_references": True})
            assert (await service.publish({"request_key": service.pending["key"], "code": CODE,
                                           "summary": "Published before native terminal."}))["valid"]
            published = True
            return SimpleNamespace(json=lambda: {"frames": []})
        return SimpleNamespace(json=lambda: {"frames": [{"kind": "interaction_complete", "cursor": 2,
                                                         "interaction_id": "cached-then-terminal"}]})

    monkeypatch.setattr("launchkit.architect._native_request", request)
    artifact = await service.generate(*inputs())
    assert artifact["native_run"]["status"] == "unknown" and service.pending["admission"]
    retire = service.native_members.retire_member
    attempts = []

    async def lost_ack(identity):
        attempts.append(identity)
        await retire(identity)
        if len(attempts) == 1:
            raise RuntimeError("Retirement acknowledgement unavailable")

    monkeypatch.setattr(service.native_members, "retire_member", lost_ack)
    with pytest.raises(RuntimeError, match="acknowledgement"):
        await service.observe_generation(artifact["key"])
    saved = copy.deepcopy(service.pending)
    assert saved["completed"] and saved["author_retirement_started"]
    restored = VisualizerService(service.architect, timeout=3, poll_interval=.001)
    result = await restored.generate(*inputs())
    assert result["native_run"]["status"] == "completed"
    assert result["code_hash"] == artifact["code_hash"] and result["native_admission"] == saved["admission"]
    assert attempts == [saved["author_identity"]] * 2
    assert len(service.native_members.ensures) == 1 and len(calls) == 3
    assert restored.pending is None


@pytest.mark.asyncio
async def test_failed_geometry_feedback_generates_revision_instead_of_returning_bad_cache(service, monkeypatch):
    inspections, prompts = [], []
    async def request(client, method, url, **kwargs):
        if method == "post":
            prompts.append(kwargs["json"]["content"])
            inspected = await service.inspect({})
            inspections.append(inspected)
            await service.inspect({"view": "overview", "request_key": service.pending["key"], "include_author_references": True})
            code = CODE if inspected["revision_feedback"] else "return {positions: []};"
            assert (await service.publish({"request_key": inspected["request_key"], "code": code,
                                           "summary": "Corrected layout" if inspected["revision_feedback"] else "Incomplete layout"}))["valid"]
            return SimpleNamespace(json=lambda: native_admission(service, "native", 1))
        return SimpleNamespace(json=lambda: {"frames": [{"kind": "interaction_complete", "interaction_id": "native", "cursor": 2}]})
    monkeypatch.setattr("launchkit.architect._native_request", request)
    original = await service.generate(*inputs())
    assert "feedback_hash" not in original
    assert (await service.generate(*inputs(), feedback=" \n"))["key"] == original["key"]
    feedback = "The animation left out configured identities."
    corrected = await service.generate(*inputs(), feedback=feedback)
    assert corrected["key"] != original["key"] and not corrected["cached"]
    assert corrected["code"] == CODE and corrected["feedback_hash"]
    assert inspections[-1]["revision_feedback"] == feedback
    assert inspections[-1]["model"] == inspections[0]["model"]
    assert inspections[-1]["blueprint"] == inspections[0]["blueprint"]
    assert "Apply the current visual refinement or reported runtime diagnostic" in prompts[-1]
    assert "Replace the previous composition freely when requested" in prompts[-1]
    assert "frozen proposal facts or executable interface" in prompts[-1]
    assert (await service.generate(*inputs(), feedback=feedback))["key"] == corrected["key"]
    assert len(prompts) == 2


def test_revision_feedback_is_bounded_diagnostic_text():
    assert checked_feedback("  Layout\x00 failed\n at\t node  ") == "Layout failed at node"
    with pytest.raises(ValueError, match="1000"):
        checked_feedback("x" * 1001)
    with pytest.raises(ValueError, match="diagnostic string"):
        checked_feedback({"instructions": "Not a diagnostic"})


@pytest.mark.asyncio
async def test_background_retry_delivers_failed_artifact_feedback_to_native_revision(service, monkeypatch):
    from launchkit.visualization_operations import VisualizationOperations
    calls = native_completion(service, monkeypatch)
    operations = VisualizationOperations()
    first = await operations.submit(service, *inputs())
    await asyncio.gather(*operations.tasks.values())
    feedback = f"Program {first['artifact']['key']} failed validation: missing node. Return corrected complete code."
    retry = await operations.submit(service, *inputs(), feedback=feedback)
    reconnected = await operations.submit(service, *inputs(), feedback=feedback)
    assert retry is reconnected and retry["id"] != first["id"]
    await asyncio.gather(*operations.tasks.values())
    assert retry["status"] == "completed"
    assert retry["artifact"]["key"] != first["artifact"]["key"]
    assert retry["artifact"]["feedback_hash"]
    assert sum(method == "post" for method, _, _ in calls) == 2


@pytest.mark.asyncio
async def test_pending_native_timeout_resumes_admission_without_duplicate_send(service, monkeypatch):
    release = asyncio.Event()
    calls = native_completion(service, monkeypatch, release=release)
    service.timeout = 0.04
    with pytest.raises(VisualizerPending, match="same request") as error:
        await service.generate(*inputs())
    assert service.pending["admission"]["interaction_id"] == "native-visualization"
    assert error.value.interaction_id == "native-visualization" and error.value.admission_confirmed
    assert error.value.request_key == service.pending["key"]
    assert not service.lock.locked()
    release.set()
    service.timeout = 3
    artifact = await service.generate(*inputs())
    assert artifact["source"] == "agent"
    assert sum(call[0] == "post" for call in calls) == 1


@pytest.mark.asyncio
async def test_operation_observes_slow_admitted_generation_and_holds_later_context_without_duplicate_send(service, monkeypatch):
    from launchkit.visualization_operations import VisualizationOperations
    release = asyncio.Event()
    calls = native_completion(service, monkeypatch, release=release)
    service.timeout = .025
    operations = VisualizationOperations(reconnect_interval=.002, reconnect_max_interval=.004)
    first = await operations.submit(service, *inputs(), stream_id="guide")
    for _ in range(15000):
        if first["status"] == "observing":
            break
        await asyncio.sleep(.002)
    assert first["status"] == "observing" and first["native_run_status"] == "unknown"
    assert first["native_interaction_id"] == "native-visualization" and "artifact" not in first
    assert "completed_at" not in first and first["reconnect_attempts"] >= 1
    assert operations.get(first["id"]) is first
    assert await operations.submit(service, *inputs(), stream_id="guide") is first
    original_key = service.pending["key"]
    original_graph = copy.deepcopy(service.pending["model"])
    changed, graph = inputs()
    changed["project"]["name"] = "Later proposal"
    later = await operations.submit(service, changed, graph, stream_id="guide")
    assert later["status"] == "queued"
    assert service.pending["key"] == original_key and service.pending["model"] == original_graph
    assert sum(method == "post" for method, _, _ in calls) == 1
    service.timeout = 3
    release.set()
    # A hang guard, not a speed budget: both generations finish through fsync'd
    # writes, which took 0.7-1.4 s here under I/O pressure.
    await asyncio.wait_for(asyncio.gather(*operations.tasks.values()), 30)
    assert first["status"] == later["status"] == "completed"
    assert first["artifact"]["key"] == original_key
    assert later["artifact"]["key"] != original_key
    assert sum(method == "post" for method, _, _ in calls) == 2
    assert "observation_status" not in first and "detail" not in first


@pytest.mark.asyncio
async def test_pending_observation_shutdown_and_restart_resume_durable_admission_without_paid_resend(service, monkeypatch):
    from launchkit.visualization_operations import VisualizationOperations
    release = asyncio.Event()
    calls = native_completion(service, monkeypatch, release=release)
    service.timeout = .025
    operations = VisualizationOperations(reconnect_interval=.01)
    receipt = await operations.submit(service, *inputs(), stream_id="guide")
    for _ in range(15000):
        if receipt["status"] == "observing":
            break
        await asyncio.sleep(.002)
    assert receipt["status"] == "observing"
    key, admission = service.pending["key"], copy.deepcopy(service.pending["admission"])
    await operations.shutdown()
    assert receipt["status"] == "interrupted" and "completed_at" not in receipt
    assert not operations.tasks and not operations.jobs
    restored = VisualizerService(service.architect, timeout=.5, poll_interval=.001)
    # Reading the saved receipt, then returning real matching frames, is the
    # recovery boundary. A new operations process has no in-memory cache.
    release.set()
    resumed_calls = native_completion(restored, monkeypatch)
    recovered = VisualizationOperations(reconnect_interval=.001)
    done = await recovered.submit(restored, *inputs(), stream_id="guide")
    await asyncio.wait_for(asyncio.gather(*recovered.tasks.values()), 1)
    assert done["status"] == "completed" and done["artifact"]["key"] == key
    assert done["artifact"]["native_run"]["interaction_id"] == admission["interaction_id"]
    assert sum(method == "post" for method, _, _ in calls) == 1
    assert all(method == "get" for method, _, _ in resumed_calls)


@pytest.mark.asyncio
async def test_actual_terminal_without_publication_is_failed_without_automatic_paid_retry(service, monkeypatch):
    from launchkit.visualization_operations import VisualizationOperations
    calls = native_completion(service, monkeypatch, publish=False)
    operations = VisualizationOperations(reconnect_interval=.001)
    receipt = await operations.submit(service, *inputs())
    await asyncio.wait_for(asyncio.gather(*operations.tasks.values()), 1)
    assert receipt["status"] == "failed" and "artifact" not in receipt
    assert service.pending is None and not operations.tasks
    assert sum(method == "post" for method, _, _ in calls) == 1


@pytest.mark.asyncio
async def test_ambiguous_admission_retries_with_same_idempotency_key(service, monkeypatch):
    completed = native_completion(service, monkeypatch)
    from launchkit.architect import _native_request
    keys = []
    async def request(client, method, url, **kwargs):
        if method == "post":
            keys.append(kwargs["json"]["idempotency_key"])
            if len(keys) == 1:
                raise TimeoutError()
        return await _native_request(client, method, url, **kwargs)
    monkeypatch.setattr("launchkit.architect._native_request", request)
    with pytest.raises(VisualizerUnavailable):
        await service.generate(*inputs())
    artifact = await service.generate(*inputs())
    assert artifact["source"] == "agent" and keys[0] == keys[1]
    assert sum(call[0] == "post" for call in completed) == 1


@pytest.mark.asyncio
async def test_simultaneous_requests_do_not_replace_frozen_model(service, monkeypatch):
    release = asyncio.Event()
    native_completion(service, monkeypatch, release=release)
    first = asyncio.create_task(service.generate(*inputs()))
    for _ in range(30000):
        if service.pending:
            break
        await asyncio.sleep(0.001)
    original = service.pending["key"]
    other, model = inputs()
    other["project"]["name"] = "Changed"
    with pytest.raises(VisualizerBusy):
        await service.generate(other, model)
    assert service.pending["key"] == original
    release.set()
    await first


@pytest.mark.asyncio
async def test_publication_requires_current_request_context_and_both_images(service, monkeypatch):
    async def request(client, method, url, **kwargs):
        if method == "post":
            inspect = await service.inspect({})
            assert inspect["model"] == inputs()[1]
            assert not (await service.publish({"request_key": "stale", "code": CODE, "summary": "Wrong"}))["valid"]
            assert not (await service.publish({"request_key": service.pending["key"], "code": CODE, "summary": "Before references"}))["valid"]
            await service.read_examples({})
            assert not (await service.publish({"request_key": service.pending["key"], "code": CODE, "summary": "Examples without images"}))["valid"]
            await service.inspect({"view": "overview", "request_key": service.pending["key"], "include_author_references": True})
            assert not (await service.publish({"request_key": service.pending["key"], "code": "```javascript\n" + CODE, "summary": "Fenced"}))["valid"]
            assert (await service.publish({"request_key": service.pending["key"], "code": CODE, "summary": "Read references"}))["valid"]
            return SimpleNamespace(json=lambda: native_admission(service, "native", 1))
        return SimpleNamespace(json=lambda: {"frames": [{"kind": "interaction_complete", "interaction_id": "native", "cursor": 2}]})
    monkeypatch.setattr("launchkit.architect._native_request", request)
    await service.generate(*inputs())


@pytest.mark.asyncio
async def test_terminal_without_publication_releases_pending_and_allows_retry(service, monkeypatch):
    native_completion(service, monkeypatch, publish=False)
    with pytest.raises(VisualizerUnavailable, match="without publishing"):
        await service.generate(*inputs())
    assert service.pending is None and not service.lock.locked()
    native_completion(service, monkeypatch)
    assert (await service.generate(*inputs()))["source"] == "agent"


@pytest.mark.asyncio
async def test_receipt_rehydrates_after_service_restart(service, monkeypatch):
    release = asyncio.Event()
    native_completion(service, monkeypatch, release=release)
    service.timeout = 0.04
    with pytest.raises(VisualizerUnavailable):
        await service.generate(*inputs())
    restored = VisualizerService(service.architect, timeout=3, poll_interval=0.001)
    calls = native_completion(restored, monkeypatch)
    artifact = await restored.generate(*inputs())
    assert artifact["source"] == "agent" and all(call[0] == "get" for call in calls)


@pytest.mark.asyncio
@pytest.mark.parametrize("prior_contract", ["visual-layout-v1", "visual-layout-v2"])
async def test_old_admission_is_drained_before_freeform_and_its_cache_is_not_reused(service, monkeypatch, prior_contract):
    release = asyncio.Event()
    native_completion(service, monkeypatch, release=release)
    service.timeout = 0.04
    with pytest.raises(VisualizerUnavailable):
        await service.generate(*inputs())
    receipt = service.pending
    # This fixture represents a legacy host, which did not write the new
    # freeform admission archive. Do not leave a second modern owner behind.
    (service.root / 'requests' / (receipt['key'] + '.json')).unlink()
    receipt.update(contract=prior_contract, key="legacy-layout-request")
    receipt.pop("author_identity")
    receipt.pop("author_session_id")
    receipt["admission"].pop("identity")
    receipt.pop("author_context_read")
    receipt.update(examples_read=True, publication={"code": CODE, "summary": "Previously accepted legacy scene"},
                   publication_receipt={"request_key": receipt["key"], "code_hash": hashlib.sha256(CODE.encode()).hexdigest(),
                       **{name: receipt[name] for name in ("blueprint_hash", "model_hash", "reference_hash")},
                       "authority": "publish_visualization"})
    service._save_pending()

    restored = VisualizerService(service.architect, timeout=3, poll_interval=0.001)
    calls = native_completion(restored, monkeypatch)
    artifact = await restored.generate(*inputs())
    assert artifact["contract"] == CONTRACT_VERSION == "freeform-animation-v3"
    assert artifact["key"] != "legacy-layout-request"
    assert [method for method, _, _ in calls] == ["get", "post", "get"]
    assert calls[0][2]["params"]["identity"] == "visualizer:main"
    assert service.native_members.retirements == [artifact["author_identity"]]
    legacy = json.loads((service.root / "legacy-layout-request.json").read_text())
    assert legacy["contract"] == prior_contract
    assert legacy["code"] == CODE
    assert not (service.root / "pending.json").exists()


@pytest.mark.asyncio
async def test_unrecognized_pending_contract_is_preserved_without_new_admission(service, monkeypatch):
    service.root.mkdir(parents=True)
    receipt = {"contract": "unknown-future-contract", "key": "do-not-overwrite"}
    (service.root / "pending.json").write_text(json.dumps(receipt))
    calls = native_completion(service, monkeypatch)
    with pytest.raises(VisualizerUnavailable, match="saved visualization request"):
        await service.generate(*inputs())
    assert json.loads((service.root / "pending.json").read_text()) == receipt
    assert calls == []


@pytest.mark.asyncio
async def test_current_native_turn_receives_composition_contract_without_changing_frozen_graph(service, monkeypatch):
    inspected = []
    native_completion(service, monkeypatch)
    original = service.inspect
    async def inspect(args):
        value = await original(args)
        inspected.append(value)
        return value
    service.inspect = inspect
    from launchkit.architect import _native_request
    async def request(client, method, url, **kwargs):
        if method == "post":
            value = await service.inspect({})
            assert "composition_guidance" in kwargs["json"]["content"]
            assert "Canvas2D" in kwargs["json"]["content"]
            assert "supersedes the old profile" in kwargs["json"]["content"]
        return await _native_request(client, method, url, **kwargs)
    monkeypatch.setattr("launchkit.architect._native_request", request)
    before = copy.deepcopy(inputs())
    await service.generate(*before)
    value = inspected[0]
    assert value["contract"] == "freeform-animation-v3"
    assert value["model"] == before[1]
    assert {"duration", "render", "hitTest", "destroy", "chapters"} <= set(value["output_contract"])
    assert not {"positions", "groups", "annotations", "paths", "events"} & set(value["output_contract"])
    assert {"blueprint", "canvas", "ctx", "inspect", "caption"} == set(value["api"])
    assert "width,height,time,progress,playing,selectedId,showTools,reducedMotion" in value["output_contract"]["render"]
    assert "conversation" in value["composition_guidance"]
    assert "output restrictions are obsolete" in value["composition_guidance"].replace("\n", " ")
    assert "phases are explanatory context" in value["composition_guidance"]
    assert "For a selected workflow (model.selectedFlowId)" in value["composition_guidance"]
    assert "result boundary" in value["composition_guidance"]
    assert "reduced-motion static view" in value["composition_guidance"]
    assert "no fixed duration, chapter sequence or layout rule" in value["composition_guidance"]
    assert before == inputs()


def test_graph_validation_preserves_every_identity_and_exact_references():
    spec, graph = inputs()
    checked_inputs(spec, graph)
    graph["nodes"].pop()
    with pytest.raises(ValueError, match="every configured"):
        checked_inputs(spec, graph)
    spec, graph = inputs()
    graph["phases"][0]["activeEdgeIds"] = ["invented"]
    with pytest.raises(ValueError, match="unknown node or edge"):
        checked_inputs(spec, graph)


def test_visualizer_has_only_its_four_scoped_tools(service):
    names = []
    service.register_tools(SimpleNamespace(register_tool=lambda name, *args, **kwargs: names.append(name)))
    assert names == ["inspect_visualization", "read_visualization_examples", "read_visualization_reference", "publish_visualization"]


def test_model_context_compacts_duplicate_class_data_without_losing_graph_authority():
    blueprint, model = inputs()
    model['nodes'][0].update(instructions='Repeated instructions', tools={'notes': 'write'}, role='Repeated class role')
    model['edges'][0]['label'] = 'A redundant display description'
    before = copy.deepcopy((blueprint, model))
    spec, graph = visualization_context(blueprint, model)
    assert [n['id'] for n in graph['nodes']] == [n['id'] for n in model['nodes']]
    assert graph['edges'][0] == {k: v for k, v in model['edges'][0].items() if k != 'label'}
    assert graph['phases'] == model['phases']
    assert spec['agent_classes'] == blueprint['agent_classes']
    assert 'instructions' not in graph['nodes'][0] and 'tools' not in graph['nodes'][0]
    assert (blueprint, model) == before


@pytest.mark.asyncio
async def test_large_overview_retains_distinct_context_and_frozen_source_while_counting_only_real_edges(service):
    job = inspected_job(service)
    before = copy.deepcopy(job)
    result = await service.inspect({})
    projected_blueprint, projected_model = visualization_context(job["blueprint"], job["model"])
    assert result["blueprint"] == projected_blueprint
    assert result["blueprint"]["agent_classes"] == projected_blueprint["agent_classes"]
    assert result["model"]["nodes"] == projected_model["nodes"]
    assert len({node["name"] for node in result["model"]["nodes"]}) == len(job["model"]["nodes"])
    members = next(item for item in result["blueprint"]["agent_classes"] if item["id"] == "research")["members"]
    assert len(members) == len({item["instructions"] for item in members}) == 80
    phase = job["model"]["phases"][0]
    assert result["model"]["phases"] == [{**phase,
        "activeEdgeIds": ["native-dependency"], "active_edges_complete": False,
        "active_edge_count": 3, "omitted_active_edge_counts": {"peer": 1, "tool": 1}}]
    assert await service.inspect({"view": "phase", "request_key": job["key"], "id": phase["id"]}) == {
        "available": True, "request_key": job["key"], "view": "phase", "phase": phase}
    phase_edges = await service.inspect({"view": "edges", "request_key": job["key"], "phase_id": phase["id"]})
    assert phase_edges["edges"] == [edge for edge in job["model"]["edges"] if edge["id"] in phase["activeEdgeIds"]]
    assert result["model"]["flows"] == projected_model["flows"]
    assert result["model"]["entrypoint"] == job["model"]["entrypoint"]
    assert result["model"]["selectedFlowId"] == job["model"]["selectedFlowId"]
    explicit = [edge for edge in projected_model["edges"] if edge["kind"] not in {"peer", "tool"}]
    assert result["model"]["edges"] == explicit
    assert [edge["target"] for edge in result["model"]["edges"] if edge["kind"] == "input"] == ["lead:main"]
    assert result["model"]["edges_complete"] is False
    assert result["model"]["edge_count"] == len(job["model"]["edges"])
    assert result["model"]["bulk_relationship_counts"] == {"peer": 80, "tool": 240}
    groups = result["model"]["relationship_groups"]
    assert sum(group["edge_count"] for group in groups) + len(explicit) == len(job["model"]["edges"])
    peer_groups = [group for group in groups if group["kind"] == "peer"]
    assert [(group["source_class"], group["edge_count"], group["source_identity_count"],
             group["target_identity_count"], group["attributes"]["directed"]) for group in peer_groups] == [
                 ("lead", 40, 1, 40, False), ("review", 40, 1, 40, True)]
    tool_groups = [group for group in groups if group["kind"] == "tool"]
    assert {group["target_node_id"] for group in tool_groups} == {"files", "archive", "notes"}
    notes = [group for group in tool_groups if group["target_node_id"] == "notes"]
    assert {(tuple(group["attributes"]["toolNames"]), group["edge_count"]) for group in notes} == {
        (("update_case",), 20), (("read_case",), 60)}
    assert all(group["attributes"]["custom_contract"] == {"revision": 1} for group in tool_groups)
    assert "do not imply all-to-all" in result["projection"]
    assert "individual assignments require exact queries" in result["projection"]
    assert job == before
    assert fingerprint(job["blueprint"]) == job["blueprint_hash"]
    assert fingerprint(job["model"]) == job["model_hash"]


def refinement_inspection_job(service, *, mismatch=None):
    blueprint, model = large_inspection_inputs()
    prior_binding = service.request_binding(blueprint, model)
    previous = {**prior_binding, "key": prior_binding["native_request_key"], "code": CODE,
                "code_hash": hashlib.sha256(CODE.encode()).hexdigest(), "summary": "The prior complete program"}
    context = revision_context(previous)
    if mismatch == "blueprint":
        blueprint["agent_classes"][1]["members"][0]["instructions"] += " Read a newly assigned source."
    elif mismatch == "model":
        model["phases"][0]["description"] = "A changed source-grounded handoff."
    elif mismatch == "reference":
        context["previous_program"]["reference_hash"] = fingerprint({"previous": "reference"})
    elif mismatch == "no_previous":
        context = revision_context()
    feedback = "Make the labels clearer while retaining the composition." if mismatch != "no_feedback" else ""
    binding = service.request_binding(blueprint, model, feedback, revision_context=context)
    service.pending = {**binding, "key": binding["native_request_key"], "blueprint": blueprint, "model": model,
                       "revision_context": context, "revision_feedback": feedback,
                       "author_context_read": False, "publication": None}
    if mismatch == "unchecked_previous":
        context["previous_program"]["summary"] = "Changed after the source binding was frozen."
    elif mismatch == "unchecked_source":
        blueprint["agent_classes"][1]["members"][0]["instructions"] += " Changed after the source binding was frozen."
    return service.pending


@pytest.mark.asyncio
@pytest.mark.parametrize("mismatch", [None, "blueprint", "model", "reference", "no_previous", "no_feedback", "unchecked_previous", "unchecked_source"])
async def test_refinement_overview_defers_only_member_instructions_for_checked_same_facts(service, mismatch):
    job = refinement_inspection_job(service, mismatch=mismatch)
    frozen = copy.deepcopy(job)
    overview = await service.inspect({})
    expected_blueprint, expected_model = visualization_context(job["blueprint"], job["model"])
    if mismatch is None:
        for item in expected_blueprint["agent_classes"]:
            item["members"] = [{key: value for key, value in member.items() if key != "instructions"}
                               for member in item["members"]]
        assert "Only blueprint.agent_classes members' instructions are deferred" in overview["projection"]
        assert "view:full and this request_key" in overview["projection"]
        assert job["revision_context"]["previous_program"]["matches_current"] is False
    else:
        assert "instructions are deferred" not in overview["projection"]
        members = next(item for item in overview["blueprint"]["agent_classes"] if item["id"] == "research")["members"]
        assert len(members) == len({member["instructions"] for member in members}) == 80
    assert overview["blueprint"] == expected_blueprint
    assert overview["model"] == inspection_graph(expected_model)
    assert overview["previous_program"] == frozen["revision_context"]["previous_program"]
    assert overview["revision_feedback"] == frozen["revision_feedback"]
    assert job["blueprint"] == frozen["blueprint"] and job["model"] == frozen["model"]
    assert job["key"] == frozen["key"] and job["revision_binding"] == frozen["revision_binding"]
    assert job["revision_context"] == frozen["revision_context"]
    full = await service.inspect({"view": "full", "request_key": job["key"]})
    assert full["blueprint"] == frozen["blueprint"] and full["model"] == frozen["model"]
    assert not (await service.inspect({"view": "full", "request_key": "old-request"}))["available"]


@pytest.mark.asyncio
async def test_same_facts_refinement_keeps_current_reference_and_publication_authority(service):
    job = refinement_inspection_job(service)
    frozen = copy.deepcopy(job)
    previous = job["revision_context"]["previous_program"]
    args = {"request_key": job["key"], "code": CODE, "summary": "Readable labels for the same exact facts"}
    assert not (await service.publish(args))["valid"]
    overview = await service.inspect({})
    assert "instructions are deferred" in overview["projection"]
    assert job["author_context_read"] is True
    assert not service._author_context_is_bound(job)
    assert not (await service.publish(args))["valid"]
    bundle = await service.inspect({"view": "overview", "request_key": job["key"], "include_author_references": True})
    assert [block["type"] for block in bundle.blocks] == ["text", "image", "image"]
    bundled_overview = json.loads(bundle.blocks[0]["text"])
    assert bundled_overview["blueprint"] == overview["blueprint"]
    assert bundled_overview["previous_program"] == previous
    assert bundled_overview["author_reference_bundle"]["image_content_ids"] == ["household", "engineering-organisation"]
    assert bundled_overview["author_reference_bundle"]["source_reference_hash"] == job["reference_hash"]
    assert service._author_context_is_bound(job)
    assert not (await service.publish({**args, "request_key": previous["request_key"]}))["valid"]
    assert (await service.publish(args))["valid"]
    assert service._publication_is_bound(job)
    assert job["publication_receipt"]["request_key"] == job["key"] != previous["request_key"]
    assert job["publication_receipt"]["revision_binding"] == frozen["revision_binding"]
    assert all(job["publication_receipt"][name] == frozen[name]
               for name in ("blueprint_hash", "model_hash", "reference_hash"))
    assert job["blueprint"] == frozen["blueprint"] and job["model"] == frozen["model"]


@pytest.mark.asyncio
async def test_exact_paged_edge_queries_reconstruct_original_rows_and_individual_assignments(service):
    job = inspected_job(service)
    before = copy.deepcopy(job)
    rows, offset = [], 0
    while True:
        page = await service.inspect({"view": "edges", "request_key": job["key"], "offset": offset, "limit": 37})
        assert page["available"] and len(page["edges"]) <= 37
        assert page["total"] == len(job["model"]["edges"])
        rows.extend(page["edges"])
        if page["next_offset"] is None:
            break
        offset = page["next_offset"]
    assert rows == job["model"]["edges"]
    assert len({edge["id"] for edge in rows}) == len(rows)
    # The overview's lead/research group is not a complete class mesh. This
    # particular member has a review peer and no lead peer.
    target = "research:member-070"
    page = await service.inspect({"view": "edges", "request_key": job["key"], "node_id": target, "kind": "peer"})
    assert page["edges"] == [edge for edge in job["model"]["edges"]
                             if edge["kind"] == "peer" and target in (edge["source"], edge["target"])]
    assert [edge["source"] for edge in page["edges"]] == ["review:main"]
    phase = job["model"]["phases"][0]
    phase_page = await service.inspect({"view": "edges", "request_key": job["key"], "phase_id": phase["id"]})
    assert phase_page["edges"] == [edge for edge in job["model"]["edges"] if edge["id"] in phase["activeEdgeIds"]]
    tool_page = await service.inspect({"view": "edges", "request_key": job["key"],
                                       "class_id": "research", "node_id": "files", "kind": "tool", "limit": 200})
    assert tool_page["edges"] == [edge for edge in job["model"]["edges"]
                                 if edge["target"] == "files" and edge["kind"] == "tool"]
    empty = await service.inspect({"view": "edges", "request_key": job["key"], "node_id": "files", "kind": "peer"})
    assert empty["edges"] == [] and empty["total"] == 0 and empty["next_offset"] is None
    assert job == before


@pytest.mark.asyncio
async def test_exact_detail_reads_preserve_original_fields_and_require_the_current_job(service):
    job = inspected_job(service)
    for view, collection in (("node", "nodes"), ("phase", "phases"), ("flow", "flows")):
        item = job["model"][collection][0]
        result = await service.inspect({"view": view, "request_key": job["key"], "id": item["id"]})
        assert result["available"] and result[view] == item
    full = await service.inspect({"view": "full", "request_key": job["key"]})
    assert full["blueprint"] == job["blueprint"] and full["model"] == job["model"]
    old_key = job["key"]
    inspected_job(service, key="next-job")
    for view in ("overview", "node", "edges", "phase", "flow", "full"):
        args = {"view": view, "request_key": old_key}
        if view in {"node", "phase", "flow"}:
            args["id"] = "irrelevant-id"
        result = await service.inspect(args)
        assert result["available"] is False and "does not match" in result["issue"]
        assert "model" not in result and "edges" not in result


@pytest.mark.asyncio
async def test_loose_edge_kinds_remain_exact_source_rows_and_do_not_crash_overview_or_queries(service):
    blueprint, model = large_inspection_inputs()
    model["edges"][0].pop("kind")
    model["edges"][1]["kind"] = ["peer"]
    model["edges"][2]["kind"] = {"custom": True}
    checked_inputs(blueprint, model)
    job = inspected_job(service, (blueprint, model))
    before = copy.deepcopy(job)
    overview = await service.inspect({})
    assert all({key: value for key, value in edge.items() if key != "label"}
               in overview["model"]["edges"] for edge in model["edges"][:3])
    assert overview["model"]["bulk_relationship_counts"] == {"peer": 77, "tool": 240}
    exact = await service.inspect({"view": "edges", "request_key": job["key"], "limit": 3})
    assert exact["edges"] == model["edges"][:3]
    peers = await service.inspect({"view": "edges", "request_key": job["key"], "kind": "peer"})
    assert peers["edges"] == model["edges"][3:80]
    assert job == before


@pytest.mark.asyncio
@pytest.mark.parametrize("flows", [None, "custom flow context", {"id": "custom"}, [None],
    [{"name": "No source ID"}], [{"id": ["custom"]}], [{"id": "duplicate"}, {"id": "duplicate"}]])
async def test_unsupported_optional_flow_details_are_preserved_and_return_a_bounded_unavailable_issue(service, flows):
    blueprint, model = large_inspection_inputs()
    model["flows"] = flows
    checked_inputs(blueprint, model)
    job = inspected_job(service, (blueprint, model))
    before = copy.deepcopy(job)
    overview = await service.inspect({})
    assert overview["model"]["flows"] == flows
    result = await service.inspect({"view": "flow", "request_key": job["key"], "id": "custom"})
    assert result["available"] is False and len(result["issue"]) < 200
    full = await service.inspect({"view": "full", "request_key": job["key"]})
    assert full["model"] == model
    assert job == before


@pytest.mark.asyncio
async def test_compact_inspection_does_not_change_native_publication_or_cache_bindings(service, monkeypatch):
    data = large_inspection_inputs()
    before = copy.deepcopy(data)
    native_completion(service, monkeypatch)
    artifact = await service.generate(*data)
    assert artifact["blueprint_hash"] == fingerprint(before[0])
    assert artifact["model_hash"] == fingerprint(before[1])
    assert artifact["publication_receipt"]["blueprint_hash"] == artifact["blueprint_hash"]
    assert artifact["publication_receipt"]["model_hash"] == artifact["model_hash"]
    expected_key = fingerprint({name: artifact[name] for name in
                               ("contract", "blueprint_hash", "model_hash", "reference_hash")})
    assert artifact["key"] == expected_key
    assert (await service.generate(*data))["key"] == expected_key
    assert data == before


@pytest.mark.asyncio
@pytest.mark.parametrize("args", [
    {"view": "edges"}, {"view": "edges", "request_key": "current-job", "offset": True},
    {"view": "edges", "request_key": "current-job", "offset": -1},
    {"view": "edges", "request_key": "current-job", "limit": 0},
    {"view": "edges", "request_key": "current-job", "limit": 201},
    {"view": "edges", "request_key": "current-job", "limit": 1.5},
    {"view": "edges", "request_key": "current-job", "node_id": "unknown"},
    {"view": "edges", "request_key": "current-job", "class_id": "unknown"},
    {"view": "edges", "request_key": "current-job", "kind": "unknown"},
    {"view": "edges", "request_key": "current-job", "phase_id": "unknown"},
    {"view": "edges", "request_key": "current-job", "offset": 9999},
    {"view": "edges", "request_key": "current-job", "node_id": ""},
    {"view": "edges", "request_key": "current-job", "node_id": "a" * 513},
    {"view": "edges", "request_key": "current-job", "node_id": ["lead:main"]},
    {"view": "node", "request_key": "current-job"},
    {"view": "node", "request_key": "current-job", "id": "unknown"},
    {"view": "overview", "offset": True}, {"view": []}, {"view": "unknown"}, [],
])
async def test_inspection_queries_reject_invalid_shapes_filters_and_paging_without_changing_the_job(service, args):
    job = inspected_job(service)
    before = copy.deepcopy(job)
    result = await service.inspect(args)
    assert result["available"] is False and result["issue"]
    assert job == before


def populated_optional_inspection_args(**used):
    # Some providers emit every schema property. Null omits an optional field before view selection.
    return dict.fromkeys(("view", "request_key", "id", "node_id", "class_id", "kind", "phase_id", "offset", "limit")) | used


@pytest.mark.asyncio
async def test_nullable_required_all_inspection_calls_keep_exact_views_and_frozen_context(service):
    job = inspected_job(service)
    before = copy.deepcopy(job)
    args = populated_optional_inspection_args()
    overview = await service.inspect(args)
    assert overview["request_key"] == job["key"]
    assert overview["model"]["edges_complete"] is False
    assert args == populated_optional_inspection_args()
    for view, item in (("node", job["model"]["nodes"][0]),
                       ("phase", job["model"]["phases"][0]),
                       ("flow", job["model"]["flows"][0])):
        args = populated_optional_inspection_args(view=view, request_key=job["key"], id=item["id"])
        original_args = copy.deepcopy(args)
        result = await service.inspect(args)
        assert result[view] == item and result["available"] is True
        assert args == original_args
    full = await service.inspect(populated_optional_inspection_args(view="full", request_key=job["key"]))
    assert full["blueprint"] == job["blueprint"] and full["model"] == job["model"]
    exact = await service.inspect(populated_optional_inspection_args(view="edges", request_key=job["key"],
        node_id="research:member-070", kind="peer", limit=1))
    assert exact["edges"] == [job["model"]["edges"][70]] and exact["total"] == 1
    assert exact["filters"] == {"node_id": "research:member-070", "kind": "peer"}
    default_page = await service.inspect(populated_optional_inspection_args(view="edges", request_key=job["key"]))
    assert default_page["edges"] == job["model"]["edges"][:100]
    assert default_page["offset"] == 0 and default_page["next_offset"] == 100
    assert job == before


@pytest.mark.asyncio
@pytest.mark.parametrize("view", ["overview", "node", "phase", "flow", "edges", "full"])
async def test_nullable_unused_fields_cannot_bypass_stale_request_keys(service, view):
    job = inspected_job(service)
    args = populated_optional_inspection_args(view=view, request_key="previous-job")
    if view in {"node", "phase", "flow"}:
        args["id"] = "an-exact-id"
    result = await service.inspect(args)
    assert result["available"] is False and "does not match" in result["issue"]
    assert "model" not in result and "edges" not in result


@pytest.mark.asyncio
@pytest.mark.parametrize("used", [
    {"view": "edges"}, {"view": "full"}, {"view": "node", "id": "lead:main"},
    {"view": "node", "request_key": "current-job"},
    {"view": "phase", "request_key": "current-job"},
    {"view": "flow", "request_key": "current-job"},
    {"view": "overview", "node_id": ["all"]}, {"view": "full", "limit": 201, "request_key": "current-job"},
    {"view": "edges", "request_key": "current-job", "node_id": "all"},
    {"view": "edges", "request_key": "current-job", "limit": True},
    {"view": "edges", "request_key": "current-job", "limit": 201},
    {"view": "overview", "unknown": None},
])
async def test_nullable_inspection_fields_do_not_supply_required_ids_keys_or_wildcards(service, used):
    job = inspected_job(service)
    before = copy.deepcopy(job)
    result = await service.inspect(populated_optional_inspection_args(**used))
    assert result["available"] is False and result["issue"]
    assert job == before


@pytest.mark.asyncio
async def test_actual_nonnull_unused_fields_do_not_change_the_selected_overview(service):
    job = inspected_job(service)
    # Exact shape from the native sixth request at console:21212, with its key replaced by this fixture's key.
    actual = {"class_id": "all", "id": "all", "kind": "all", "limit": 1, "node_id": "all", "offset": 0,
              "phase_id": "all", "request_key": job["key"], "view": "overview"}
    result = await service.inspect(actual)
    assert result["request_key"] == job["key"] and result["model"]["edges_complete"] is False
    nullable = {key: value if key in {"view", "request_key"} else None for key, value in actual.items()}
    inspected = await service.inspect(nullable)
    assert inspected == result
    assert actual["node_id"] == "all" and actual["limit"] == 1


@pytest.mark.asyncio
async def test_all_eight_actual_native_calls_use_only_the_selected_view_without_crossing_requests(service):
    spec = starter_blueprint().model_dump(mode="json")
    owner = next(item for item in spec["agent_classes"] if item["id"] == "lead")
    owner.update(id="repo-gateway", members=[{"key": "architecture", "name": "Gateway architecture",
                                             "instructions": "Own the Gateway architectural context."}])
    spec["topology"]["entrypoint"] = "repo-gateway:architecture"
    for flow in spec["flows"]:
        for step in flow["steps"]:
            if step["agent"] == "lead:main":
                step["agent"] = "repo-gateway:architecture"
    blueprint = Blueprint.model_validate(spec).model_dump(mode="json")
    model = {"nodes": [{"id": identity, "name": identity, "kind": "agent"}
                       for identity in Blueprint.model_validate(blueprint).identities()],
             "edges": [], "phases": [{"id": "conversation-entry", "activeNodeIds": ["repo-gateway:architecture"],
                                      "activeEdgeIds": []}]}
    key = "6c87b403f1b61ef3ad17d162421334a4c60825682f0a00e860ff52614aa2a8f9"
    job = inspected_job(service, checked_inputs(blueprint, model), key=key)
    before = copy.deepcopy(job)
    common = {"class_id": "all", "id": "all", "kind": "all", "limit": 1, "node_id": "all", "offset": 0,
              "phase_id": "all", "request_key": key, "view": "overview"}
    # Exact argument shapes at console:21212, 21217, 21222, 21227, 21233, 21238, 21243 and 21248.
    calls = [common.copy(), common | {"view": "full"},
             common | {name: " " for name in ("class_id", "id", "kind", "node_id", "phase_id")},
             common | {"limit": 200},
             common | {"class_id": "repo-gateway", "id": "repo-gateway:architecture", "kind": "agent",
                       "node_id": "repo-gateway:architecture", "phase_id": "conversation-entry", "view": "node"},
             common | {"limit": 200, "view": "full"}, common | {"limit": 200}, common.copy()]
    for args in calls:
        original_args = copy.deepcopy(args)
        result = await service.inspect(args)
        assert result.get("available", True)
        if args["view"] == "node":
            assert result["node"] == model["nodes"][0]
        elif args["view"] == "full":
            assert result["blueprint"] == blueprint and result["model"] == model
        else:
            assert result["request_key"] == key and result["model"]["nodes"] == model["nodes"]
        stale = await service.inspect(args | {"request_key": "wrong-key"})
        assert stale["available"] is False and "does not match" in stale["issue"]
        assert "model" not in stale and "node" not in stale
        assert args == original_args and job == before


@pytest.mark.asyncio
async def test_inapplicable_fields_still_obey_global_bounds_and_relevant_filters_keep_and_semantics(service):
    job = inspected_job(service)
    before = copy.deepcopy(job)
    for overrides in ({"id": True}, {"class_id": "x" * 513}, {"node_id": ""}, {"offset": -1},
                      {"limit": 201}, {"limit": 1.5}, {"unknown": None}):
        result = await service.inspect({"view": "overview", "request_key": job["key"]} | overrides)
        assert result["available"] is False
    exact = await service.inspect(populated_optional_inspection_args(view="edges", request_key=job["key"],
        id="unused-detail-id", class_id="research", node_id="research:member-070", kind="peer", limit=1))
    assert exact["edges"] == [job["model"]["edges"][70]]
    no_match = await service.inspect(populated_optional_inspection_args(view="edges", request_key=job["key"],
        node_id="research:member-070", kind="peer", class_id="lead"))
    assert no_match["edges"] == [] and no_match["total"] == 0
    wildcard = await service.inspect(populated_optional_inspection_args(view="edges", request_key=job["key"], node_id="all"))
    assert wildcard["available"] is False
    assert job == before


@pytest.mark.asyncio
async def test_current_inspection_schema_and_handlers_cross_fresh_and_resumed_sdk_build_callbacks(service):
    from meerkat_meta.agent_builder import CallbackDispatcher
    dispatcher = CallbackDispatcher()
    class SessionBuilder:
        async def build_agent(self, draft):
            service.register_tools(draft)
    dispatcher.register_builder(SessionBuilder())
    job = inspected_job(service)
    for scope, resume in (("fresh-build", None), ("restored-build", "saved-native-session")):
        options = {"scope_id": scope, "profile_name": "visualizer"}
        if resume:
            options["resume_session_id"] = resume
        result = await dispatcher.handle_callback("callback/build_agent", {"options": options})
        definition = next(tool for tool in result["tools"] if tool["name"] == "inspect_visualization")
        schema = definition["input_schema"]
        assert schema["properties"]["view"]["enum"] == ["overview", "node", "edges", "phase", "flow", "full", "author_draft", None]
        assert all("null" in property_schema["type"] for property_schema in schema["properties"].values())
        assert schema["properties"]["limit"]["maximum"] == 200
        if resume:
            assert result["resume_session_id"] == resume
        queried = (await dispatcher.handle_callback("callback/call_tool", {"scope_id": scope,
            "tool": "inspect_visualization", "arguments": populated_optional_inspection_args(
                view="edges", request_key=job["key"], limit=1)}))["content"]
        assert queried["total"] == len(job["model"]["edges"])
        assert queried["edges"] == job["model"]["edges"][:1]


@pytest.mark.asyncio
async def test_acknowledged_program_is_available_before_native_tail_terminal(service, monkeypatch):
    calls = []
    async def request(client, method, url, **kwargs):
        calls.append(method)
        if method == "post":
            return SimpleNamespace(json=lambda: native_admission(service, "native", 1))
        await service.inspect({"view": "overview", "request_key": service.pending["key"], "include_author_references": True})
        assert (await service.publish({"request_key": service.pending["key"], "code": CODE, "summary": "Published scene"}))["valid"]
        return SimpleNamespace(json=lambda: {"frames": []})
    monkeypatch.setattr("launchkit.architect._native_request", request)
    # A hang guard, not a speed budget: calls == ["post", "get"] below proves the
    # program returned after one timeline read, before any terminal frame. The
    # reference reads and fsync'd writes took up to 0.9 s here under I/O pressure.
    artifact = await asyncio.wait_for(service.generate(*inputs()), 30)
    assert artifact["code"] == CODE and artifact["native_run"]["status"] == "unknown"
    assert artifact["publication_receipt"]["authority"] == "publish_visualization"
    assert service.pending["admission"]["interaction_id"] == "native"
    assert not service.pending.get("completed")
    assert (await service.generate(*inputs()))["cached"] and calls == ["post", "get"]


@pytest.mark.asyncio
async def test_restart_recovers_old_publication_from_exact_native_ack_without_paid_send(service, monkeypatch):
    import hashlib
    native_completion(service, monkeypatch, release=asyncio.Event())
    service.timeout = .02
    with pytest.raises(VisualizerUnavailable):
        await service.generate(*inputs())
    job = service.pending
    job.pop("author_identity")
    job.pop("author_session_id")
    job.pop("author_context_read", None)
    job.update(publication={"code": CODE, "summary": "Accepted before host restart"}, examples_read=True,
               cursor="console:1200", admission={"cursor": "console:10", "interaction_id": "native", "session_id": "saved-session"})
    job.pop("publication_receipt", None)
    job.pop("owner_id", None)
    service._save_pending()
    restored = VisualizerService(service.architect, timeout=.5, poll_interval=.001)
    reads = []
    async def request(client, method, url, **kwargs):
        assert method == "get"
        after = kwargs["params"]["after"]
        reads.append(after)
        if after == "console:10":
            frames = [{"kind": "text_delta", "interaction_id": "native", "cursor": f"console:{n}"} for n in range(11, 511)]
        elif after == "console:510":
            frames = [{"kind": "tool_result_received", "interaction_id": "native", "cursor": "console:511",
                       "payload": {"name": "publish_visualization", "content": [{"type": "text", "text": json.dumps({
                           "valid": True, "request_key": job["key"], "code_hash": hashlib.sha256(CODE.encode()).hexdigest()})}]}}]
        else:
            frames = []
        return SimpleNamespace(json=lambda: {"frames": frames})
    monkeypatch.setattr("launchkit.architect._native_request", request)
    async def status_post(client, url, **kwargs):
        assert url.endswith("/console/rpc") and kwargs["json"]["method"] == "meta/member_status"
        return SimpleNamespace(raise_for_status=lambda: None, json=lambda: {"result": {
            "current_session_id": "saved-session", "progress": {"run_state": "idle", "in_flight_work": 0}}})
    monkeypatch.setattr("httpx.AsyncClient.post", status_post)
    artifact = await restored.generate(*inputs())
    assert reads == ["console:10", "console:510"]
    assert artifact["native_run"]["status"] == "unknown"
    assert artifact["publication_receipt"]["native_cursor"] == "console:511"
    assert artifact["publication_receipt"]["authority"] == "native_publish_acknowledgment"
    assert restored.pending and not restored.pending.get("completed")
    observed = await restored.observe_generation(artifact["key"])
    assert reads == ["console:10", "console:510", "console:511"]
    assert observed["native_run"]["status"] == "interrupted"
    assert restored.pending is None and not (restored.root / "pending.json").exists()


@pytest.mark.asyncio
@pytest.mark.parametrize("mismatch", ["interaction", "request", "code", "blueprint", "model", "reference"])
async def test_recovered_publication_cannot_rebind_another_request_or_source(service, monkeypatch, mismatch):
    import hashlib
    native_completion(service, monkeypatch, release=asyncio.Event())
    service.timeout = .02
    with pytest.raises(VisualizerUnavailable):
        await service.generate(*inputs())
    job = service.pending
    job.update(publication={"code": CODE, "summary": "Saved output"}, examples_read=True)
    job.pop("publication_receipt", None)
    if mismatch == "blueprint":
        job["blueprint"]["project"]["name"] = "Changed proposal"
    elif mismatch == "model":
        job["model"]["nodes"][0]["name"] = "Changed graph"
    elif mismatch == "reference":
        job["reference_hash"] = "different-reference"
    service._save_pending()
    restored = VisualizerService(service.architect, timeout=.2, poll_interval=.001)
    calls = 0
    async def request(client, method, url, **kwargs):
        nonlocal calls
        assert method == "get"
        calls += 1
        result = {"valid": True, "request_key": "other" if mismatch == "request" else job["key"],
                  "code_hash": "wrong" if mismatch == "code" else hashlib.sha256(CODE.encode()).hexdigest()}
        frames = [{"kind": "tool_result_received", "interaction_id": "other" if mismatch == "interaction" else "native-visualization",
                   "cursor": 2, "payload": {"name": "publish_visualization", "result": result}}] if calls == 1 else []
        return SimpleNamespace(json=lambda: {"frames": frames})
    monkeypatch.setattr("launchkit.architect._native_request", request)
    message = "saved visualization request" if mismatch == "reference" else "matching acknowledged"
    with pytest.raises(VisualizerUnavailable, match=message):
        await restored.generate(*inputs())
    assert (restored.pending is None if mismatch == "reference" else bool(restored.pending))
    assert not restored._cache(job["key"])


@pytest.mark.asyncio
@pytest.mark.parametrize("session,run_state,in_flight", [("another-session", "idle", 0), ("saved", "running", 1), ("saved", "idle", 1)])
async def test_restore_idle_requires_exact_session_and_no_native_work(service, session, run_state, in_flight):
    job = {"owner_id": "prior-host", "admission": {"session_id": "saved", "interaction_id": "native"}}
    async def post(*args, **kwargs):
        return SimpleNamespace(raise_for_status=lambda: None, json=lambda: {"result": {
            "current_session_id": session, "progress": {"run_state": run_state, "in_flight_work": in_flight}}})
    await service._observe_restored_idle(SimpleNamespace(post=post), "http://native", job)
    assert not job.get("completed") and "native_run" not in job


def revision_context(previous=None, *, setup="setup-A", stream="overview"):
    return {"version": "prior-program-v1", "setup_id": setup, "stream_id": stream,
            "previous_program": None if previous is None else {
                "operation_id": fingerprint({"historical": previous["key"]}), "request_key": previous["key"],
                **{name: previous[name] for name in ("contract", "blueprint_hash", "model_hash", "reference_hash",
                                                    "code_hash", "code", "summary")}, "matches_current": False}}


async def editable_job(service, code=CODE):
    blueprint, model = checked_inputs(*inputs())
    prior_binding = service.request_binding(blueprint, model)
    previous = {**prior_binding, "key": prior_binding["native_request_key"], "code": code,
                "code_hash": hashlib.sha256(code.encode()).hexdigest(), "summary": "The prior complete program"}
    context = revision_context(previous)
    blueprint["project"]["description"] = "Changed context for the continuing proposed system."
    binding = service.request_binding(blueprint, model, revision_context=context)
    service.pending = {**binding, "key": binding["native_request_key"], "blueprint": blueprint, "model": model,
                       "revision_context": context, "author_context_read": False, "examples_read": False, "publication": None}
    service.root.mkdir(parents=True, exist_ok=True)
    service._save_pending()
    await service.inspect({"view": "overview", "request_key": service.pending["key"], "include_author_references": True})
    return service.pending


def source_edit_args(job, **overrides):
    return {"request_key": job["key"], "summary": "An updated complete program",
            "base_code_hash": job["revision_context"]["previous_program"]["code_hash"],
            "edits": [{"find": "duration:36", "replace": "duration:42"}], **overrides}


@pytest.mark.asyncio
@pytest.mark.parametrize("has_previous", [False, True])
@pytest.mark.parametrize("nullable_fields", [False, True])
async def test_full_program_publication_remains_available_with_or_without_revision_source(service, has_previous, nullable_fields):
    job = await editable_job(service)
    if not has_previous:
        context = revision_context()
        binding = service.request_binding(job["blueprint"], job["model"], revision_context=context)
        job.update(binding, key=binding["native_request_key"], revision_context=context)
        await service.inspect({"view": "overview", "request_key": job["key"], "include_author_references": True})
    args = {"request_key": job["key"], "code": CODE, "summary": "A freely composed complete program"}
    if nullable_fields:
        args.update(base_code_hash=None, edits=None)
    result = await service.publish(args)
    assert result["valid"] is True
    assert job["publication"] == {"code": CODE, "summary": args["summary"]}
    assert job["publication_receipt"]["authority"] == "publish_visualization"
    assert job["publication_receipt"]["revision_binding"] == job["revision_binding"]
    assert service._publication_is_bound(job)


@pytest.mark.asyncio
@pytest.mark.parametrize("reverse", [False, True])
async def test_source_edits_apply_to_original_spans_and_publish_complete_current_source(service, reverse):
    job = await editable_job(service)
    frozen = copy.deepcopy(job["revision_context"])
    edits = [{"find": "duration:36", "replace": "duration:42"},
             {"find": "return {", "replace": "return /* duration:36 remains historical */ {"}]
    if reverse:
        edits.reverse()
    result = await service.publish(source_edit_args(job, code=None, edits=edits))
    expected = CODE.replace("duration:36", "duration:42").replace("return {", edits[0 if reverse else 1]["replace"])
    assert result == {"valid": True, "request_key": job["key"], "code_hash": hashlib.sha256(expected.encode()).hexdigest(),
                      "validation": "Program captured. The isolated host checks duration and render, then executes the animation. Factual and visual quality need review of the actual scene."}
    assert job["revision_context"] == frozen
    assert job["publication"] == {"code": expected, "summary": "An updated complete program"}
    assert job["publication_receipt"]["request_key"] != frozen["previous_program"]["request_key"]
    assert job["publication_receipt"]["code_hash"] != frozen["previous_program"]["code_hash"]
    assert service._publication_is_bound(job)
    saved = json.loads((service.root / "pending.json").read_text())
    assert saved["publication"]["code"] == expected and "edits" not in saved["publication"]


@pytest.mark.asyncio
async def test_source_edits_allow_adjacent_original_spans_and_empty_replacements(service):
    source = "/* remove */" + CODE
    job = await editable_job(service, source)
    result = await service.publish(source_edit_args(job, edits=[
        {"find": "/* remove */", "replace": ""}, {"find": "return {", "replace": "return {"}]))
    assert result["valid"] and job["publication"]["code"] == CODE


@pytest.mark.asyncio
@pytest.mark.parametrize("overrides", [
    {"request_key": "prior-or-other-request"},
    {"base_code_hash": "0" * 64}, {"base_code_hash": None}, {"base_code_hash": True},
    {"edits": None}, {"edits": []}, {"edits": "not-an-array"},
    {"edits": [None]}, {"edits": [{"find": "duration:36"}]},
    {"edits": [{"find": "duration:36", "replace": "duration:42", "count": 1}]},
    {"edits": [{"find": "", "replace": "anything"}]},
    {"edits": [{"find": True, "replace": "anything"}]},
    {"edits": [{"find": "duration:36", "replace": None}]},
    {"edits": [{"find": "duration:99", "replace": "duration:42"}]},
    {"edits": [{"find": "frame", "replace": "tick"}]},
    {"edits": [{"find": "duration:36", "replace": "duration:42"},
               {"find": "duration:36", "replace": "duration:48"}]},
    {"edits": [{"find": "return {duration", "replace": "return {duration"},
               {"find": "duration:36", "replace": "duration:42"}]},
    {"edits": [{"find": "duration:36", "replace": "duration:42"},
               {"find": "duration:42", "replace": "duration:48"}]},
    {"code": CODE}, {"code": ""}, {"code": False},
    {"summary": ""}, {"summary": None}, {"extra": "not an accepted tool field"},
    {"edits": [{"find": CODE, "replace": ""}]},
    {"edits": [{"find": CODE, "replace": " " * 20}]},
    {"edits": [{"find": CODE, "replace": "```javascript\n" + CODE}]},
    {"edits": [{"find": "duration:36", "replace": "x" * 48000}]},
    {"edits": [{"find": "duration:36", "replace": "x" * 48001}]},
])
async def test_source_edit_rejections_preserve_existing_memory_and_saved_publication(service, overrides):
    job = await editable_job(service)
    assert (await service.publish({"request_key": job["key"], "code": CODE, "summary": "Existing accepted source"}))["valid"]
    before = copy.deepcopy(job)
    saved_before = (service.root / "pending.json").read_bytes()
    result = await service.publish(source_edit_args(job, **overrides))
    assert result["valid"] is False
    assert job == before and (service.root / "pending.json").read_bytes() == saved_before


@pytest.mark.asyncio
@pytest.mark.parametrize("mutation", [
    lambda job: job.pop("revision_context"),
    lambda job: job.pop("revision_binding"),
    lambda job: job["revision_context"].update(previous_program=None),
    lambda job: job["revision_context"].update(setup_id="another-setup"),
    lambda job: job["revision_context"].update(stream_id="another-view"),
    lambda job: job["revision_context"]["previous_program"].update(code=CODE.replace("36", "42")),
    lambda job: job["revision_context"]["previous_program"].update(matches_current=True),
    lambda job: job.update(key="1" * 64),
    lambda job: job["blueprint"]["project"].update(description="Changed after the frozen request"),
    lambda job: job["model"]["nodes"][0].update(name="Changed after the frozen request"),
])
async def test_source_edits_require_the_exact_frozen_base_and_current_request_binding(service, mutation):
    job = await editable_job(service)
    args = source_edit_args(job)
    mutation(job)
    args["request_key"] = job["key"]
    service._save_pending()
    before = copy.deepcopy(job)
    saved_before = (service.root / "pending.json").read_bytes()
    result = await service.publish(args)
    assert result["valid"] is False
    assert job == before and (service.root / "pending.json").read_bytes() == saved_before


@pytest.mark.asyncio
async def test_source_edit_publication_schema_and_handler_cross_fresh_and_resumed_callbacks(service):
    from meerkat_meta.agent_builder import CallbackDispatcher
    dispatcher = CallbackDispatcher()
    class SessionBuilder:
        async def build_agent(self, draft):
            service.register_tools(draft)
    dispatcher.register_builder(SessionBuilder())
    job = await editable_job(service)
    for scope, resume in (("fresh-edit-build", None), ("resumed-edit-build", "saved-native-session")):
        options = {"scope_id": scope, "profile_name": "visualizer"}
        if resume:
            options["resume_session_id"] = resume
        built = await dispatcher.handle_callback("callback/build_agent", {"options": options})
        definition = next(tool for tool in built["tools"] if tool["name"] == "publish_visualization")
        schema = definition["input_schema"]
        assert schema["required"] == ["request_key", "summary"]
        assert all("null" in schema["properties"][field]["type"] for field in ("code", "base_code_hash", "edits"))
        assert schema["properties"]["edits"]["items"]["additionalProperties"] is False
        result = (await dispatcher.handle_callback("callback/call_tool", {"scope_id": scope,
            "tool": "publish_visualization", "arguments": source_edit_args(job, code=None)}))["content"]
        assert result["valid"] is True
        assert job["publication"]["code"] == CODE.replace("duration:36", "duration:42")


@pytest.mark.asyncio
async def test_materialized_source_edit_recovers_exact_native_ack_after_restart_without_new_send(service, monkeypatch):
    native_completion(service, monkeypatch)
    previous = await service.generate(*inputs())
    context = revision_context(previous)
    blueprint, model = inputs()
    blueprint["project"]["description"] = "A small change to the same continuing system."
    async def admit(*args):
        pass
    sends = []
    async def request(client, method, url, **kwargs):
        if method == "post":
            sends.append(kwargs["json"])
            assert "base_code_hash and exact edits:[{find,replace}]" in kwargs["json"]["content"]
            inspection = await service.inspect({})
            assert "base_code_hash" in inspection["revision_source"]
            assert "all applied to the original source" in inspection["composition_guidance"]
            return SimpleNamespace(json=lambda: native_admission(service, "edited-native", 10))
        await service.inspect({"view": "overview", "request_key": service.pending["key"], "include_author_references": True})
        result = await service.publish(source_edit_args(service.pending, code=None))
        assert result["valid"] is True
        return SimpleNamespace(json=lambda: {"frames": []})
    monkeypatch.setattr("launchkit.architect._native_request", request)
    artifact = await service.generate_continuous_with_native_admission(blueprint, model, revision_context=context,
        on_admission=admit, on_native_admission=admit)
    assert len(sends) == 1 and artifact["code"] == CODE.replace("duration:36", "duration:42")
    assert artifact["publication_receipt"]["authority"] == "publish_visualization"
    assert artifact["native_admission"]["interaction_id"] == "edited-native"
    job = service.pending
    job.pop("publication_receipt")
    service._save_pending()
    (service.root / (job["key"] + ".json")).unlink()
    restored = VisualizerService(service.architect, timeout=.5, poll_interval=.001)
    async def reconnect(client, method, url, **kwargs):
        assert method == "get", "Recovery must not pay for another native generation."
        return SimpleNamespace(json=lambda: {"frames": [
            {"kind": "tool_result_received", "interaction_id": "edited-native", "cursor": 11,
             "payload": {"name": "publish_visualization", "result": {
                 "valid": True, "request_key": job["key"], "code_hash": artifact["code_hash"]}}},
            {"kind": "interaction_complete", "interaction_id": "edited-native", "cursor": 12}]})
    monkeypatch.setattr("launchkit.architect._native_request", reconnect)
    recovered = await restored.generate_continuous_with_native_admission(blueprint, model, revision_context=context,
        on_admission=admit, on_native_admission=admit)
    assert recovered["code"] == artifact["code"] and recovered["key"] == artifact["key"]
    assert recovered["publication_receipt"]["authority"] == "native_publish_acknowledgment"
    assert recovered["publication_receipt"]["revision_binding"] == artifact["revision_binding"]
    assert recovered["publication_receipt"]["native_cursor"] == 11
    assert recovered["native_admission"] == artifact["native_admission"]
    assert restored.pending is None


def test_revision_source_binding_separates_setup_view_and_program_bytes(service):
    blueprint, model = inputs()
    contexts = [revision_context(), revision_context(setup="setup-B"), revision_context(stream="selected-flow")]
    bindings = [service.request_binding(blueprint, model, revision_context=context) for context in contexts]
    assert len({item["native_request_key"] for item in bindings}) == 3
    assert {item["model_hash"] for item in bindings} == {fingerprint(model)}
    assert all(item["revision_binding"] == revision_context_binding(context)
               for item, context in zip(bindings, contexts))
    assert "revision_binding" not in service.request_binding(blueprint, model)


@pytest.mark.asyncio
async def test_checked_prior_program_is_frozen_advisory_source_not_current_publication(service, monkeypatch):
    native_completion(service, monkeypatch)
    previous = await service.generate(*inputs())
    context = revision_context(previous)
    frozen = copy.deepcopy(context)
    blueprint, model = inputs()
    blueprint["project"]["description"] = "A changed task with the same continuing owners."
    expected = service.request_binding(blueprint, model, revision_context=context)
    new_code = CODE.replace("duration:36", "duration:42")
    sends, admissions = [], []
    async def admit():
        admissions.append("host")
    async def native_admit(key, admission):
        admissions.append((key, admission["interaction_id"]))
    async def request(client, method, url, **kwargs):
        if method == "post":
            sends.append(kwargs["json"])
            inspection = await service.inspect({})
            assert inspection["previous_program"] == frozen["previous_program"]
            assert inspection["blueprint"] == visualization_context(blueprint, model)[0]
            assert service.pending["publication"] is None
            assert "checked previous_program" in kwargs["json"]["content"]
            assert not (await service.publish({"request_key": previous["key"], "code": CODE,
                                              "summary": "Cannot publish the prior request again"}))["valid"]
            context["previous_program"]["code"] = "caller replaced the queued program"
            await service.inspect({"view": "overview", "request_key": service.pending["key"], "include_author_references": True})
            assert (await service.publish({"request_key": inspection["request_key"], "code": new_code,
                                           "summary": "The updated program"}))["valid"]
            return SimpleNamespace(json=lambda: native_admission(service, "new-revision", 10))
        return SimpleNamespace(json=lambda: {"frames": [{"kind": "interaction_complete", "cursor": 11,
                                                         "interaction_id": "new-revision"}]})
    monkeypatch.setattr("launchkit.architect._native_request", request)
    artifact = await service.generate_continuous_with_native_admission(blueprint, model, revision_context=context,
        on_admission=admit, on_native_admission=native_admit)
    assert artifact["key"] == expected["native_request_key"] != previous["key"]
    assert artifact["code"] == new_code and artifact["blueprint_hash"] == fingerprint(blueprint)
    assert artifact["publication_receipt"]["revision_binding"] == expected["revision_binding"]
    assert admissions == ["host", (artifact["key"], "new-revision")] and len(sends) == 1
    archive = service.root / "requests" / (artifact["key"] + ".json")
    saved = json.loads(archive.read_text())
    assert saved["revision_binding"] == expected["revision_binding"]
    assert CODE not in archive.read_text() and "previous_program" not in saved
    assert service._cache(artifact["key"])["code"] == new_code
    async def no_send(*args, **kwargs):
        raise AssertionError("Exact reconnect must reuse its original scoped publication.")
    monkeypatch.setattr("launchkit.architect._native_request", no_send)
    cached = await service.generate_continuous_with_native_admission(blueprint, model, revision_context=frozen,
        on_admission=admit, on_native_admission=native_admit)
    assert cached["cached"] is True and cached["key"] == artifact["key"]
    assert admissions == ["host", (artifact["key"], "new-revision")]


@pytest.mark.parametrize("mutation", [
    lambda context: context.update(version="unknown"),
    lambda context: context.update(stream_id=""),
    lambda context: context.update(setup_id="bad\nsetup"),
    lambda context: context["previous_program"].update(code="return an altered program without the same hash;"),
    lambda context: context["previous_program"].update(contract="visual-layout-v2"),
    lambda context: context["previous_program"].update(matches_current=True),
    lambda context: context["previous_program"].update(request_key="../another-context"),
])
def test_malformed_or_rebound_prior_context_is_rejected_before_native_work(service, mutation):
    blueprint, model = inputs()
    artifact = {**service.request_binding(blueprint, model), "key": fingerprint({"old": "request"}),
                "code": CODE, "code_hash": hashlib.sha256(CODE.encode()).hexdigest(), "summary": "Prior program"}
    context = revision_context(artifact)
    mutation(context)
    with pytest.raises(ValueError):
        service.request_binding(blueprint, model, revision_context=context)
    assert service.pending is None and service.architect.starts == 0


@pytest.mark.asyncio
async def test_restored_pending_cannot_substitute_another_frozen_revision_source(service, monkeypatch):
    calls = native_completion(service, monkeypatch, release=asyncio.Event())
    service.timeout = .02
    context = revision_context()
    async def admission(*args):
        pass
    with pytest.raises(VisualizerPending):
        await service.generate_continuous_with_native_admission(*inputs(), revision_context=context,
            on_admission=admission, on_native_admission=admission)
    source_key = service.pending["key"]
    service.pending["revision_context"]["setup_id"] = "another-setup"
    service._save_pending()
    restored = VisualizerService(service.architect, timeout=.02, poll_interval=.001)
    starts, count = service.architect.starts, len(calls)
    with pytest.raises(VisualizerUnavailable, match="saved visualization request"):
        await restored.generate_continuous_with_native_admission(*inputs(), revision_context=context,
            on_admission=admission, on_native_admission=admission)
    assert service.architect.starts == starts and len(calls) == count
    assert (service.root / "pending.json").exists() and not restored._cache(source_key)


@pytest.mark.asyncio
@pytest.mark.parametrize("mutation", ["receipt_binding", "admission", "archive"])
async def test_scoped_program_cache_requires_its_exact_publication_and_saved_native_admission(service, monkeypatch, mutation):
    native_completion(service, monkeypatch)
    context = revision_context()
    async def admission(*args):
        pass
    artifact = await service.generate_continuous_with_native_admission(*inputs(), revision_context=context,
        on_admission=admission, on_native_admission=admission)
    path = service.root / (artifact["key"] + ".json")
    saved = json.loads(path.read_text())
    if mutation == "receipt_binding":
        saved["publication_receipt"]["revision_binding"]["setup_id"] = "another-setup"
        path.write_text(json.dumps(saved))
    elif mutation == "admission":
        saved["native_admission"]["interaction_id"] = "another-interaction"
        path.write_text(json.dumps(saved))
    else:
        for record_path in (service.root / "requests").glob(artifact["key"] + "*.json"):
            record = json.loads(record_path.read_text())
            record["revision_binding"]["stream_id"] = "another-view"
            record_path.write_text(json.dumps(record))
    assert service._cache(artifact["key"]) is None


@pytest.mark.asyncio
@pytest.mark.parametrize("missing", [("revision_binding",), ("revision_context",),
                                    ("revision_binding", "revision_context")])
async def test_incomplete_restored_revision_is_preserved_without_native_dispatch(service, monkeypatch, missing):
    blueprint, model = checked_inputs(*inputs())
    context = revision_context()
    binding = service.request_binding(blueprint, model, revision_context=context)
    service.pending = {**binding, "key": binding["native_request_key"], "blueprint": blueprint,
                       "model": model, "revision_context": context, "admission": None,
                       "publication": None, "examples_read": False, "idempotency_key": "saved-attempt"}
    for name in missing:
        service.pending.pop(name)
    service.root.mkdir(mode=0o700)
    service._save_pending()
    before = (service.root / "pending.json").read_bytes()
    calls = native_completion(service, monkeypatch)
    restored = VisualizerService(service.architect, timeout=.02, poll_interval=.001)
    async def admit(*args):
        raise AssertionError("A damaged saved revision cannot admit new work")
    with pytest.raises(VisualizerUnavailable, match="saved visualization request"):
        await restored.generate_continuous_with_native_admission(blueprint, model, revision_context=context,
            on_admission=admit, on_native_admission=admit)
    assert service.architect.starts == 0 and calls == [] and restored.pending is None
    assert (service.root / "pending.json").read_bytes() == before
