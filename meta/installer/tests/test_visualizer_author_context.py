"""One checked author-context delivery, without native or animation execution."""
import asyncio
import base64
from copy import deepcopy
import hashlib
import json
import threading

import pytest

from launchkit import visualization_review as references
from launchkit import visualizer as module
from launchkit.projects import fingerprint
from meerkat_meta import ToolResultContent
from meerkat_meta.agent_builder import CallbackDispatcher
from test_visualizer import CODE, inputs, large_inspection_inputs, native_admission, native_completion, revision_context, service, view_scope_inputs
from test_visualizer_author_references import active_job, callbacks


def bundle_arguments(job):
    return {"view": "overview", "request_key": job["key"], "include_author_references": True}


@pytest.mark.asyncio
async def test_native_request_labels_and_saved_session_fence_fresh_and_restored_tool_scopes(service):
    from test_visualizer import editable_job
    job = await editable_job(service)
    job.update(author_identity="visualizer-request-" + "a" * 32, author_session_id="owned-native-session")
    service._save_pending()
    labels = {"visualization_author_identity": job["author_identity"], "visualization_request_key": job["key"]}
    _built, owner = await callbacks(service, labels=labels, session_id=job["author_session_id"])
    _built, legacy = await callbacks(service)
    _built, foreign = await callbacks(service, labels={**labels, "visualization_author_identity": "visualizer-request-" + "b" * 32})
    _built, replaced = await callbacks(service, labels=labels, session_id="replacement-native-session")
    before = deepcopy(job)
    for call in (legacy, foreign, replaced):
        refused = await call("inspect_visualization", bundle_arguments(job))
        assert refused["content"]["available"] is False
        refused = await call("publish_visualization", {"request_key": job["key"], "code": CODE, "summary": "Foreign scope"})
        assert refused["content"]["valid"] is False
    assert job == before
    result = await owner("inspect_visualization", bundle_arguments(job))
    assert [block["type"] for block in result["content_blocks"]] == ["text", "image", "image"]
    published = await owner("publish_visualization", {"request_key": job["key"], "code": CODE, "summary": "Owned scope"})
    assert published["content"]["valid"] is True
    # A rebuilt exact native session keeps its owner labels and tool authority.
    _built, restored = await callbacks(service, labels=labels, session_id=job["author_session_id"])
    assert (await restored("inspect_visualization", {"view": "full", "request_key": job["key"]}))["content"]["model"] == job["model"]
    next_job = deepcopy(job)
    next_job.update(author_identity="visualizer-request-" + "c" * 32, author_session_id="next-native-session")
    service.pending = next_job
    before = deepcopy(next_job)
    assert (await owner("inspect_visualization", {}))["content"]["available"] is False
    assert (await restored("publish_visualization", {"request_key": next_job["key"], "code": CODE, "summary": "Late scope"}))["content"]["valid"] is False
    assert next_job == before


@pytest.mark.asyncio
@pytest.mark.parametrize("notification_first", [True, False], ids=["created-before-successor", "delayed-predecessor"])
async def test_installed_after_create_binds_only_unique_fresh_scope_and_never_a_successor(service, notification_first):
    from test_visualizer import editable_job
    job = await editable_job(service)
    job.update(author_identity="visualizer-request-" + "d" * 32, author_session_id="actual-created-session")
    service._save_pending()
    labels = {"visualization_author_identity": job["author_identity"], "visualization_request_key": job["key"]}
    dispatcher = CallbackDispatcher()

    class Builder:
        async def build_agent(self, draft):
            service.register_tools(draft)

        async def after_create(self, session_id, context):
            service.author_created(session_id, context)

    dispatcher.register_builder(Builder())

    async def build(scope, **options):
        await dispatcher.handle_callback("callback/build_agent", {"options": {
            "scope_id": scope, "profile_name": "visualizer", "labels": labels, **options}})

    async def call(scope, tool="inspect_visualization", args=None):
        return await dispatcher.handle_callback("callback/call_tool", {
            "scope_id": scope, "tool": tool, "arguments": args or bundle_arguments(job)})

    async def created(session_id):
        await dispatcher.handle_callback("callback/after_create", {"session_id": session_id, "labels": labels})

    await build("first-mint")
    before = deepcopy(job)
    assert (await call("first-mint"))["content"]["available"] is False
    assert job == before
    if notification_first:
        await created(job["author_session_id"])
        assert "content_blocks" in await call("first-mint")
    await build("successor-mint")
    await created(job["author_session_id"])
    await created("replacement-session")
    assert (await call("successor-mint"))["content"]["available"] is False
    if notification_first:
        assert "content_blocks" in await call("first-mint")
    else:
        assert (await call("first-mint"))["content"]["available"] is False
        assert job == before
    assert (await call("successor-mint", "publish_visualization", {
        "request_key": job["key"], "code": CODE, "summary": "Unproven successor"}))["content"]["valid"] is False
    # Cold restoration supplies the SDK's exact resume session directly and
    # does not depend on a mint notification or inherit its ambiguity.
    await build("exact-restored", resume_session_id=job["author_session_id"])
    assert "content_blocks" in await call("exact-restored")
    assert (await call("exact-restored", "publish_visualization", {
        "request_key": job["key"], "code": CODE, "summary": "Exact restored owner"}))["content"]["valid"] is True


def unpack(result):
    assert isinstance(result, ToolResultContent)
    assert [block["type"] for block in result.blocks] == ["text", "image", "image"]
    return json.loads(result.blocks[0]["text"]), result.blocks[1:]


@pytest.mark.asyncio
async def test_overview_removes_only_verified_duplicate_names_and_flows_with_original_detail_available(service):
    blueprint, model = large_inspection_inputs()
    model["flows"] = deepcopy(blueprint["flows"])
    member_node = next(node for node in model["nodes"] if node["id"] == "research:member-000")
    member_node["name"] = "Distinct map label for the first context"
    blueprint, model = module.checked_inputs(blueprint, model)
    binding = service.request_binding(blueprint, model)
    key = binding.pop("native_request_key")
    job = service.pending = {**binding, "key": key, "blueprint": blueprint, "model": model}
    original = deepcopy((blueprint, model))

    overview, images = unpack(await service.inspect(bundle_arguments(job)))
    projected = next(item for item in overview["blueprint"]["agent_classes"] if item["id"] == "research")
    original_class = next(item for item in blueprint["agent_classes"] if item["id"] == "research")
    assert projected["members"][0] == original_class["members"][0]
    assert all("name" not in member for member in projected["members"][1:])
    assert [member["key"] for member in projected["members"]] == [member["key"] for member in original_class["members"]]
    assert [member["instructions"] for member in projected["members"]] == [member["instructions"] for member in original_class["members"]]
    assert projected["tools"] == original_class["tools"]
    assert overview["model"]["nodes"] == module.visualization_context(blueprint, model)[1]["nodes"]
    assert "flows" not in overview["model"] and overview["blueprint"]["flows"] == model["flows"]
    assert len(images) == 2
    assert await service.inspect({"view": "node", "request_key": key, "id": member_node["id"]}) == {
        "available": True, "request_key": key, "view": "node", "node": member_node}
    assert await service.inspect({"view": "flow", "request_key": key, "id": model["flows"][0]["id"]}) == {
        "available": True, "request_key": key, "view": "flow", "flow": model["flows"][0]}
    assert await service.inspect({"view": "full", "request_key": key}) == {
        "available": True, "request_key": key, "view": "full", "blueprint": blueprint, "model": model}
    assert (blueprint, model) == original
    assert (await service.publish({"request_key": key, "code": CODE, "summary": "Original facts retained."}))["valid"]
    assert service.architect.starts == 0


def test_distinct_flow_context_and_json_types_are_never_removed_as_duplicates():
    blueprint, model = large_inspection_inputs()
    source = deepcopy(blueprint["flows"][0])
    model["flows"] = [{**source, "stepCount": 2, "layerCount": 2, "valid": True}]
    projected = module.visualization_context(blueprint, model)[1]["flows"]
    assert projected == [{"id": source["id"], "stepCount": 2, "layerCount": 2, "valid": True}]
    assert [{**source, **flow} for flow in projected] == model["flows"]
    model["flows"][0].update(description="Distinct explanation in the model", steps=["examine", "review"])
    projected = module.visualization_context(blueprint, model)[1]["flows"]
    assert projected[0]["description"] == model["flows"][0]["description"]
    assert projected[0]["steps"] == model["flows"][0]["steps"]
    assert [{**source, **flow} for flow in projected] == model["flows"]
    model["flows"][0]["id"] = "different-flow"
    assert module.visualization_context(blueprint, model)[1]["flows"] == model["flows"]
    model["flows"] = [{**source}, {**source, "valid": True}]
    assert module.visualization_context(blueprint, model)[1]["flows"] == model["flows"], "Ambiguous IDs retain all detail."
    blueprint["flows"].append({**source})
    model["flows"] = [{**source, "valid": True}]
    assert module.visualization_context(blueprint, model)[1]["flows"] == model["flows"], "Ambiguous blueprint IDs also retain detail."
    blueprint["flows"] = [{"id": "typed-context", "steps": [True]}]
    model["flows"] = [{"id": "typed-context", "steps": [1]}]
    assert module.visualization_context(blueprint, model)[1]["flows"] == model["flows"]


@pytest.mark.asyncio
@pytest.mark.parametrize("draft_primary", [False, True], ids=["retained-inline-request", "paged-primary-draft"])
async def test_primary_draft_omits_only_inline_prior_code_without_changing_replay_or_publication_checks(service, draft_primary):
    blueprint, model = inputs()
    prior_code = CODE + "\r\n// Historical caf\u00e9 scene.\r\n"
    draft_code = CODE.replace("duration:36", "duration:43") + "\r\n// Current supplied composition."
    previous = {**service.request_binding(blueprint, model), "code": prior_code,
                "code_hash": hashlib.sha256(prior_code.encode()).hexdigest(), "summary": "An older checked scene."}
    previous["key"] = previous["native_request_key"]
    context = revision_context(previous)
    digest = hashlib.sha256(draft_code.encode()).hexdigest()
    feedback = "Retain the supplied composition. draft-sha256:" + digest
    binding = service.request_binding(blueprint, model, feedback, revision_context=context)
    key = binding.pop("native_request_key")
    job = service.pending = {**binding, "key": key, "blueprint": blueprint, "model": model,
        "revision_context": context, "revision_feedback": feedback,
        "author_draft": {"code": draft_code, "code_hash": digest},
        **({"author_draft_delivery": "inspection-v1"} if draft_primary else {})}
    frozen = deepcopy(job)
    replay = module.visualization_request_message(job)
    args = {"request_key": key, "author_draft_code_hash": digest, "summary": "Verified supplied composition."}
    assert (await service.publish(args))["valid"] is False
    overview, images = unpack(await service.inspect(bundle_arguments(job)))
    expected_prior = context["previous_program"]
    assert overview["previous_program"] == ({name: value for name, value in expected_prior.items() if name != "code"}
                                           if draft_primary else expected_prior)
    assert ("view:full" in overview["revision_source"]) is draft_primary
    assert len(images) == 2
    assert (await service.publish(args))["valid"] is False, "The smaller overview cannot replace complete draft reads."
    page = await service.inspect({"view": "author_draft", "request_key": key})
    assert page["code"] == draft_code and page["next_offset"] is None
    assert (await service.publish(args))["valid"] is True, "No optional older source read is required for draft publication."
    complete = await service.inspect({"view": "full", "request_key": key})
    assert complete["blueprint"] == blueprint and complete["model"] == model
    if draft_primary:
        assert complete["previous_program"] == expected_prior
    else:
        assert "previous_program" not in complete
    assert module.visualization_request_message(job) == replay
    for name in ("key", "blueprint", "model", "revision_binding", "revision_context", "author_draft", "feedback_hash"):
        assert job[name] == frozen[name]
    assert service.architect.starts == 0


@pytest.mark.asyncio
async def test_installed_dispatcher_delivers_exact_existing_overview_and_both_original_images_with_examples_on_demand(service):
    service._load()
    job = active_job(service)
    binding = service.request_binding(job["blueprint"], job["model"])
    original = deepcopy(job)
    default = await service.inspect({"request_key": job["key"]})
    built, call = await callbacks(service)
    inspect_tool = next(tool for tool in built["tools"] if tool["name"] == "inspect_visualization")
    option = inspect_tool["input_schema"]["properties"]["include_author_references"]
    assert option["type"] == ["boolean", "null"]
    assert {tool["name"] for tool in built["tools"]} == {
        "inspect_visualization", "read_visualization_examples", "read_visualization_reference", "publish_visualization"}
    result = await call("inspect_visualization", bundle_arguments(job))
    assert "content_blocks" in result and "content" not in result
    blocks = result["content_blocks"]
    assert [block["type"] for block in blocks] == ["text", "image", "image"]
    overview = json.loads(blocks[0]["text"])
    bundle = overview.pop("author_reference_bundle")
    assert overview == default
    examples = module.visualization_examples()
    assert "source_examples" not in bundle
    assert bundle["source_examples_tool"] == "read_visualization_examples"
    assert bundle["source_examples_hash"] == fingerprint(examples)
    assert module.reference_content_hash(fingerprint(examples),
        default["reference_context"]["images"]) == bundle["source_reference_hash"] == job["reference_hash"]
    assert bundle["image_content_ids"] == [image["id"] for image in default["reference_context"]["images"]]
    for descriptor, image in zip(default["reference_context"]["images"], blocks[1:]):
        assert image["media_type"] == "image/png"
        data = base64.b64decode(image["data"], validate=True)
        assert hashlib.sha256(data).hexdigest() == descriptor["png_hash"]
        assert data == (module.REFERENCE_ROOT / ("household-reference.png" if descriptor["id"] == "household"
                        else "engineering-reference.png")).read_bytes()
    assert service.request_binding(job["blueprint"], job["model"]) == binding
    delivery = {"observation": "host_returned_tool_content", "request_key": job["key"],
        "source_reference_hash": job["reference_hash"],
        "images": [{"id": image["id"], "png_hash": image["png_hash"]}
                   for image in default["reference_context"]["images"]]}
    assert job == {**original, "author_context_read": True, "author_reference_delivery": delivery}
    saved = json.loads((service.root / "pending.json").read_text())
    assert saved == job and "png_base64" not in json.dumps(saved) and "source_examples" not in saved
    assert service.saved_request(job["key"])["author_reference_delivery"] == delivery
    delivered_examples = (await call("read_visualization_examples", {}))["content"]
    assert delivered_examples["examples"] == examples and job["examples_read"] is True
    assert service.architect.starts == 0


@pytest.mark.asyncio
async def test_default_false_and_null_inspection_stay_json_and_do_not_unlock_publication(service):
    service._load()
    job = active_job(service)
    expected = await service.inspect({})
    original = deepcopy(job)
    assert job["author_context_read"] is True and "author_reference_delivery" not in job
    for args in ({"include_author_references": False}, {"include_author_references": None},
                 {"request_key": job["key"], "include_author_references": False}):
        assert await service.inspect(args) == expected
        assert job == original
    assert (await service.publish({"request_key": job["key"], "code": CODE, "summary": "Offline code string."}))["valid"] is False
    assert "examples_read" not in job and service.architect.starts == 0


@pytest.mark.asyncio
async def test_installed_resume_build_callback_registers_bundle_schema_for_retained_host_request(service):
    service._load()
    job = active_job(service)
    service._save_pending()
    restored = module.VisualizerService(service.architect)
    restored._load()
    assert restored.pending == job
    dispatcher = CallbackDispatcher()

    class Builder:
        async def build_agent(self, draft):
            assert draft.profile_name == "visualizer"
            assert draft.resume_session_id == "00000000-0000-4000-8000-000000000001"
            restored.register_tools(draft)

    dispatcher.register_builder(Builder())
    built = await dispatcher.handle_callback("callback/build_agent", {"options": {
        "scope_id": "offline-resumed-author", "profile_name": "visualizer",
        "resume_session_id": "00000000-0000-4000-8000-000000000001"}})
    tool = next(item for item in built["tools"] if item["name"] == "inspect_visualization")
    assert tool["input_schema"]["properties"]["include_author_references"]["type"] == ["boolean", "null"]
    assert built["resume_session_id"] == "00000000-0000-4000-8000-000000000001"
    delivered = await dispatcher.handle_callback("callback/call_tool", {
        "scope_id": "offline-resumed-author", "tool": "inspect_visualization",
        "arguments": bundle_arguments(restored.pending)})
    assert [block["type"] for block in delivered["content_blocks"]] == ["text", "image", "image"]
    assert json.loads(delivered["content_blocks"][0]["text"])["request_key"] == job["key"]
    assert restored.pending["author_context_read"] is True
    assert "examples_read" not in restored.pending and "author_context_read" not in job
    assert service.architect.starts == 0


@pytest.mark.asyncio
async def test_successful_bundle_satisfies_publication_gate_without_optional_source_tool_call(service, monkeypatch):
    service._load()
    job = active_job(service)

    async def forbidden_duplicate(_):
        raise AssertionError("Optional JavaScript references are not required for publication.")

    monkeypatch.setattr(service, "read_examples", forbidden_duplicate)
    overview, _images = unpack(await service.inspect(bundle_arguments(job)))
    assert overview["request_key"] == job["key"] and job["author_context_read"] is True
    assert "examples_read" not in job
    published = await service.publish({"request_key": job["key"], "code": CODE, "summary": "Captured offline body only."})
    assert published["valid"] is True
    assert published["code_hash"] == hashlib.sha256(CODE.encode()).hexdigest()
    assert job["publication_receipt"]["reference_hash"] == overview["author_reference_bundle"]["source_reference_hash"]
    job["admission"] = {"interaction_id": "offline-author", "cursor": 1}
    artifact = service._published_artifact(job)
    assert artifact["author_reference_delivery"] == job["author_reference_delivery"]
    assert service._cache(job["key"])["author_reference_delivery"] == artifact["author_reference_delivery"]
    from launchkit.visualization_timeline import publication
    receipt = {"native_request_key": job["key"], "blueprint_hash": job["blueprint_hash"],
               "model_hash": job["model_hash"], "artifact": artifact}
    assert publication(receipt)["author_reference_delivery"] == artifact["author_reference_delivery"]
    wrong_request = deepcopy(artifact)
    wrong_request["author_reference_delivery"]["request_key"] = "0" * 64
    projected = publication({**receipt, "artifact": wrong_request})
    assert projected["available"] and "author_reference_delivery" not in projected
    legacy = deepcopy(artifact)
    legacy.pop("author_reference_delivery")
    (service.root / (job["key"] + ".json")).write_text(json.dumps(legacy))
    assert "author_reference_delivery" not in service._cache(job["key"])
    assert service.architect.starts == 0


@pytest.mark.asyncio
async def test_original_image_reads_require_a_checked_factual_context_before_publication(service):
    job = active_job(service)
    for identity in ("household", "engineering-organisation"):
        await service.read_reference({"request_key": job["key"], "reference_id": identity})
    assert "author_context_read" not in job and "examples_read" not in job
    args = {"request_key": job["key"], "code": CODE, "summary": "Offline body only."}
    assert (await service.publish(args))["valid"] is False
    await service.inspect({"request_key": job["key"]})
    assert job["author_context_read"] is True
    assert (await service.publish(args))["valid"] is True
    assert service.architect.starts == 0


@pytest.mark.asyncio
@pytest.mark.parametrize("mutation", [
    lambda job: job.pop("author_context_read"),
    lambda job: job.update(author_context_read=False),
    lambda job: job.pop("author_reference_delivery"),
    lambda job: job["author_reference_delivery"].update(request_key="0" * 64),
    lambda job: job["author_reference_delivery"].update(source_reference_hash="0" * 64),
    lambda job: job["author_reference_delivery"]["images"].pop(),
    lambda job: job["author_reference_delivery"]["images"][0].update(png_hash="invalid"),
    lambda job: job["model"]["nodes"][0].update(name="Changed after context delivery"),
])
async def test_publication_rejects_missing_or_stale_checked_context_and_image_delivery(service, mutation):
    job = active_job(service)
    unpack(await service.inspect(bundle_arguments(job)))
    mutation(job)
    before = deepcopy(job)
    assert (await service.publish({"request_key": job["key"], "code": CODE,
        "summary": "Rejected incomplete delivery."}))["valid"] is False
    assert job == before and not job.get("publication")
    assert service.architect.starts == 0


@pytest.mark.asyncio
async def test_bundle_preserves_full_large_proposal_design_context_feedback_and_previous_view_source(service):
    service._load()
    blueprint, model = view_scope_inputs(503, "briefing")
    additional = deepcopy(next(item for item in blueprint["agent_classes"] if item["id"] == "research"))
    additional.update(id="additional", name="Additional continuing contexts", instances=105)
    blueprint["agent_classes"].append(additional)
    model["nodes"].extend({"id": identity, "name": identity, "kind": "agent"}
                          for identity in module.AgentClass.model_validate(additional).identities())
    blueprint, model = module.checked_inputs(blueprint, model)
    prior_binding = service.request_binding(blueprint, model)
    previous = {**prior_binding, "key": prior_binding["native_request_key"], "code": CODE,
                "code_hash": hashlib.sha256(CODE.encode()).hexdigest(), "summary": "Prior proposal only."}
    context = revision_context(previous, setup="current-setup", stream="current-setup:briefing")
    model["design_context"] = {
        "facts": {"ownership": {"summary": "Each continuing case retains its own context.",
                                "status": "confirmed", "source_message_ids": ["user-ownership"]},
                  "audience": {"summary": "The wider audience is unresolved.",
                               "status": "unresolved", "source_message_ids": []}},
        "requirements": [{"id": "later-interface", "outcome": "A later interface for additional people.",
                          "disposition": "deferred", "source_message_ids": ["user-interface"],
                          "decision_message_ids": ["user-later"]}]}
    feedback = module.checked_feedback("Keep case ownership visible.")
    binding = service.request_binding(blueprint, model, feedback, revision_context=context)
    key = binding.pop("native_request_key")
    job = service.pending = {**binding, "key": key, "blueprint": blueprint, "model": model,
                             "revision_feedback": feedback, "revision_context": context}
    original = deepcopy(job)
    default = await service.inspect({"request_key": key})
    overview, _images = unpack(await service.inspect(bundle_arguments(job)))
    overview.pop("author_reference_bundle")
    assert overview == default
    assert overview["model"]["design_context"] == model["design_context"]
    assert overview["model"]["selectedFlowId"] == "briefing"
    assert len([node for node in overview["model"]["nodes"] if node["kind"] == "agent"]) == 610
    assert overview["previous_program"] == context["previous_program"]
    assert overview["revision_feedback"] == feedback
    assert {name: value for name, value in job.items() if name != "author_reference_delivery"} == {**original, "author_context_read": True}
    published = await service.publish({"request_key": key, "code": CODE, "summary": "Offline source capture."})
    assert published["valid"] is True
    assert job["publication_receipt"]["revision_binding"] == binding["revision_binding"]
    assert service.architect.starts == 0


@pytest.mark.asyncio
@pytest.mark.parametrize("args", [
    {"include_author_references": True},
    {"view": "full", "request_key": "current", "include_author_references": True},
    {"view": "edges", "request_key": "current", "include_author_references": True},
    {"request_key": "foreign", "include_author_references": True},
    *({"request_key": "current", "include_author_references": value} for value in [1, 0, "true", [], {}]),
])
async def test_invalid_bundle_arguments_cannot_load_pixels_or_unlock_publication(service, monkeypatch, args):
    job = active_job(service)
    if args.get("request_key") == "current": args = {**args, "request_key": job["key"]}

    def forbidden(*_):
        raise AssertionError("Rejected bundle must not read pixels.")

    monkeypatch.setattr(references, "bundled_review_reference_context", forbidden)
    result = await service.inspect(args)
    assert result["available"] is False and "examples_read" not in job
    assert not service.root.exists() and service.architect.starts == 0


@pytest.mark.asyncio
@pytest.mark.parametrize("fault", ["absent", "missing", "source", "contract", "blueprint", "model", "feedback"])
async def test_missing_or_inconsistent_bundle_has_no_partial_context_or_publication_authority(service, monkeypatch, fault):
    job = active_job(service)
    args = bundle_arguments(job)
    if fault == "absent": service.pending = None
    elif fault == "missing":
        def unavailable(*_): raise OSError("Synthetic missing reference.")
        monkeypatch.setattr(references, "bundled_review_reference_context", unavailable)
    elif fault == "source": job["reference_hash"] = "0" * 64
    elif fault == "contract": job["contract"] = "visual-layout-v2"
    elif fault == "blueprint": job["blueprint"]["project"]["name"] = "Different proposal"
    elif fault == "model": job["model"]["nodes"][0]["name"] = "Different context"
    else: job["revision_feedback"] = "Changed unsourced findings"
    result = await service.inspect(args)
    assert result["available"] is False
    assert set(result) <= {"available", "reason", "issue"}
    assert "examples_read" not in job and not service.root.exists()
    assert service.architect.starts == 0


@pytest.mark.asyncio
@pytest.mark.parametrize("fault", ["clear", "replace-same-key", "replace-other-key", "key", "reference",
                                     "contract", "blueprint", "model", "feedback"])
async def test_delayed_bundle_cannot_mix_replaced_or_mutated_request_context(service, monkeypatch, fault):
    job = active_job(service)
    loaded = references.bundled_review_reference_context()
    started, release = threading.Event(), threading.Event()

    def delayed(*_):
        started.set()
        if not release.wait(3): raise AssertionError("Offline reference fixture was not released.")
        return deepcopy(loaded)

    monkeypatch.setattr(references, "bundled_review_reference_context", delayed)
    task = asyncio.create_task(service.inspect(bundle_arguments(job)))
    try:
        assert await asyncio.to_thread(started.wait, 30)
        if fault == "clear": service.pending = None
        elif fault == "replace-same-key": service.pending = deepcopy(job)
        elif fault == "replace-other-key": service.pending = {**deepcopy(job), "key": "1" * 64}
        elif fault == "key": job["key"] = "1" * 64
        elif fault == "reference": job["reference_hash"] = "1" * 64
        elif fault == "contract": job["contract"] = "visual-layout-v2"
        elif fault == "blueprint": job["blueprint"]["project"]["name"] = "New proposal"
        elif fault == "model": job["model"]["nodes"][0]["name"] = "New model"
        else: job["revision_feedback"] = "New feedback"
    finally:
        release.set()
    result = await task
    assert result["available"] is False and set(result) <= {"available", "issue"}
    assert "examples_read" not in job
    assert service.pending is None or "examples_read" not in service.pending
    assert not service.root.exists() and service.architect.starts == 0


@pytest.mark.asyncio
async def test_source_examples_changed_after_reference_check_do_not_deliver_mixed_bundle(service, monkeypatch):
    job = active_job(service)
    loaded = references.bundled_review_reference_context()
    monkeypatch.setattr(references, "bundled_review_reference_context", lambda *_: deepcopy(loaded))
    examples = module.visualization_examples()
    monkeypatch.setattr(module, "visualization_examples", lambda: {**examples, "renderer": examples["renderer"] + "\n// Changed source"})
    result = await service.inspect(bundle_arguments(job))
    assert result["available"] is False and "examples_read" not in job
    assert "source_examples" not in result and not service.root.exists()


@pytest.mark.asyncio
@pytest.mark.parametrize("fault", ["missing-binding", "missing-context", "version", "setup", "stream"])
async def test_bundle_rejects_unbound_or_mixed_revision_context(service, fault):
    job = active_job(service)
    context = revision_context(setup="current-setup", stream="current-view")
    binding = service.request_binding(job["blueprint"], job["model"], revision_context=context)
    key = binding.pop("native_request_key")
    job.update(binding, key=key, revision_context=context)
    args = bundle_arguments(job)
    if fault == "missing-binding": job.pop("revision_binding")
    elif fault == "missing-context": job.pop("revision_context")
    elif fault == "version": context["version"] = "unknown"
    elif fault == "setup": context["setup_id"] = "foreign-setup"
    else: context["stream_id"] = "foreign-view"
    result = await service.inspect(args)
    assert result["available"] is False and "examples_read" not in job
    assert not service.root.exists() and service.architect.starts == 0


@pytest.mark.asyncio
@pytest.mark.parametrize("field,value", [("version", "unknown"), ("setup_id", "foreign-setup"),
                                        ("stream_id", "foreign-view")])
async def test_revision_context_change_during_pixel_load_cannot_deliver_mixed_author_bundle(service, monkeypatch, field, value):
    job = active_job(service)
    context = revision_context(setup="current-setup", stream="current-view")
    binding = service.request_binding(job["blueprint"], job["model"], revision_context=context)
    key = binding.pop("native_request_key")
    job.update(binding, key=key, revision_context=context)
    loaded = references.bundled_review_reference_context()
    started, release = threading.Event(), threading.Event()

    def delayed(*_):
        started.set()
        if not release.wait(3): raise AssertionError("Offline reference fixture was not released.")
        return deepcopy(loaded)

    monkeypatch.setattr(references, "bundled_review_reference_context", delayed)
    task = asyncio.create_task(service.inspect(bundle_arguments(job)))
    try:
        assert await asyncio.to_thread(started.wait, 30)
        context[field] = value
    finally:
        release.set()
    result = await task
    assert result["available"] is False and "examples_read" not in job
    assert not service.root.exists() and service.architect.starts == 0


@pytest.mark.asyncio
async def test_failed_bundle_receipt_write_does_not_unlock_publication(service, monkeypatch):
    service._load()
    job = active_job(service)
    await service.read_reference({"request_key": job["key"], "reference_id": "household"})
    previous = deepcopy(job)

    def failed_save(): raise OSError("Synthetic receipt failure.")

    monkeypatch.setattr(service, "_save_pending", failed_save)
    with pytest.raises(OSError, match="Synthetic receipt failure"):
        await service.inspect(bundle_arguments(job))
    assert job == previous and "examples_read" not in job
    assert json.loads((service.root / "pending.json").read_text()) == previous
    assert (await service.publish({"request_key": job["key"], "code": CODE, "summary": "No delivery."}))["valid"] is False
    assert service.architect.starts == 0


@pytest.mark.asyncio
@pytest.mark.parametrize("has_prior_program", [False, True], ids=["fresh", "feedback-and-prior-source"])
async def test_compact_send_delivers_one_checked_bundle_and_preserves_request_and_revision(service, monkeypatch, has_prior_program):
    from types import SimpleNamespace
    import uuid

    assert module.AUTHOR_REFERENCE_GUIDANCE not in module.VISUALIZER_PROMPT
    assert module.VISUALIZER_PROMPT.endswith(module.COMPOSITION_GUIDANCE)
    blueprint, model = inputs()
    previous = None
    if has_prior_program:
        prior = service.request_binding(blueprint, model)
        previous = {**prior, "key": prior["native_request_key"], "code": CODE,
                    "code_hash": hashlib.sha256(CODE.encode()).hexdigest(), "summary": "Prior design."}
    context = revision_context(previous)
    feedback = "Keep the continuing context visible." if has_prior_program else ""
    frozen = deepcopy((blueprint, model, context))
    expected = service.request_binding(blueprint, model, feedback, revision_context=context)
    posts, bundles = [], []
    call = None

    async def no_duplicate(_args):
        raise AssertionError("The one checked bundle supplies the required context and original images.")

    monkeypatch.setattr(service, "read_examples", no_duplicate)
    monkeypatch.setattr(service, "read_reference", no_duplicate)

    async def request(client, method, url, **kwargs):
        nonlocal call
        if method == "post":
            body = deepcopy(kwargs["json"])
            posts.append(body)
            assert set(body) == {"identity", "content", "origin", "idempotency_key"}
            assert body["identity"] == service.pending["author_identity"] and body["origin"] == "launch-visualizer"
            assert body["idempotency_key"] == service.pending["idempotency_key"]
            assert str(uuid.UUID(body["idempotency_key"])) == body["idempotency_key"]
            content = body["content"]
            assert f"request {expected['native_request_key']}" in content
            assert f"request_key:{expected['native_request_key']}" in content
            assert "view:overview" in content and "include_author_references:true" in content
            assert "composition_guidance" in content and "view_guidance" in content and "reference_guidance" in content
            assert "compatible separate tools" in content and "do not repeat successful image reads" in content
            assert "unchanged JavaScript examples and original renderer are optional" in content
            assert module.AUTHOR_REFERENCE_GUIDANCE not in content
            assert module.LIFECYCLE_GUIDANCE not in content and module.RESPONSIVE_POPULATION_GUIDANCE not in content
            assert "not a JSON layout" in content and "Compose freely" in content
            assert ("Apply the current visual refinement or reported runtime diagnostic" in content) is has_prior_program
            assert ("checked previous_program" in content) is has_prior_program
            if has_prior_program:
                assert "base_code_hash and exact edits:[{find,replace}]" in content
                assert "Replace the previous composition freely when requested" in content
                assert "frozen proposal facts or executable interface" in content
                assert "Other conversation requests are not factual input" in content
            _built, call = await callbacks(service, labels=service.native_members.ensures[-1][2]["labels"],
                                          session_id=service.pending["author_session_id"])
            return SimpleNamespace(json=lambda: native_admission(service, "compact-author"))
        delivered = await call("inspect_visualization", bundle_arguments(service.pending))
        blocks = delivered["content_blocks"]
        assert [block["type"] for block in blocks] == ["text", "image", "image"]
        overview = json.loads(blocks[0]["text"])
        bundles.append(overview)
        assert overview["request_key"] == expected["native_request_key"]
        assert overview["composition_guidance"] == module.COMPOSITION_GUIDANCE
        assert overview["reference_guidance"] == module.AUTHOR_REFERENCE_GUIDANCE
        assert overview["view_guidance"] == module.composition_view_guidance(blueprint, model)
        assert overview["previous_program"] == context["previous_program"] and overview["revision_feedback"] == feedback
        assert "source_examples" not in overview["author_reference_bundle"]
        assert overview["author_reference_bundle"]["source_examples_tool"] == "read_visualization_examples"
        assert overview["author_reference_bundle"]["source_examples_hash"] == fingerprint(module.visualization_examples())
        assert overview["author_reference_bundle"]["source_reference_hash"] == expected["reference_hash"]
        assert overview["author_reference_bundle"]["image_content_ids"] == ["household", "engineering-organisation"]
        for descriptor, block in zip(overview["reference_context"]["images"], blocks[1:]):
            assert hashlib.sha256(base64.b64decode(block["data"], validate=True)).hexdigest() == descriptor["png_hash"]
        published = await call("publish_visualization", {"request_key": overview["request_key"],
                                                        "code": CODE, "summary": "Offline transport fixture only."})
        assert published["content"]["valid"] is True
        return SimpleNamespace(json=lambda: {"frames": [{"kind": "interaction_complete", "cursor": 2,
                                                         "interaction_id": "compact-author"}]})

    monkeypatch.setattr("launchkit.architect._native_request", request)
    async def admission(*_args):
        pass

    artifact = await service.generate_continuous_with_native_admission(blueprint, model, feedback=feedback,
        revision_context=context, on_admission=admission, on_native_admission=admission)
    assert len(posts) == len(bundles) == 1
    assert artifact["key"] == expected["native_request_key"] and artifact["code"] == CODE
    assert artifact["author_reference_delivery"]["request_key"] == artifact["key"]
    assert artifact["publication_receipt"]["revision_binding"] == expected["revision_binding"]
    assert service.request_binding(blueprint, model, feedback, revision_context=context) == expected
    assert (blueprint, model, context) == frozen
    cached = await service.generate_continuous_with_native_admission(blueprint, model, feedback=feedback,
        revision_context=context, on_admission=admission, on_native_admission=admission)
    assert cached["cached"] and cached["key"] == artifact["key"] and len(posts) == len(bundles) == 1
