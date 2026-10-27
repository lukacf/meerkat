"""Checked author image exposure offline, without model or animation execution."""
import asyncio
import base64
from copy import deepcopy
import hashlib
import json
import shutil
import threading

import pytest

from launchkit import visualization_review as references
from launchkit import visualizer as module
from launchkit.projects import fingerprint
from meerkat_meta.agent_builder import CallbackDispatcher
from test_visualizer import CODE, inputs, native_completion, service


def active_job(service):
    blueprint, model = inputs()
    binding = service.request_binding(blueprint, model)
    key = binding.pop("native_request_key")
    service.pending = {**binding, "key": key, "blueprint": blueprint, "model": model}
    return service.pending


async def callbacks(service, *, labels=None, session_id=None):
    dispatcher = CallbackDispatcher()

    class Builder:
        async def build_agent(self, draft):
            service.register_tools(draft)

    dispatcher.register_builder(Builder())
    scope = "offline-author-references"
    built = await dispatcher.handle_callback("callback/build_agent", {
        "options": {"scope_id": scope, "profile_name": "visualizer", "labels": labels or {}, "session_id": session_id}})

    async def call(name, args):
        return await dispatcher.handle_callback("callback/call_tool", {
            "scope_id": scope, "tool": name, "arguments": args})

    return built, call


@pytest.mark.asyncio
async def test_installed_dispatcher_exposes_original_pixels_separately_with_request_and_source_binding(service):
    service._load()
    job = active_job(service)
    before = deepcopy(job)
    built, call = await callbacks(service)
    tool = next(item for item in built["tools"] if item["name"] == "read_visualization_reference")
    assert tool["input_schema"]["required"] == ["request_key", "reference_id"]
    assert tool["input_schema"]["additionalProperties"] is False
    inspected = (await call("inspect_visualization", {}))["content"]
    context = inspected["reference_context"]
    assert context["available"] is True and context["request_key"] == job["key"]
    assert context["source_reference_hash"] == job["reference_hash"]
    assert context["evidence_scope"] == "user-supplied-design-reference-pixels"
    assert "png_base64" not in json.dumps(inspected)
    assert "read_visualization_reference" in inspected["reference_guidance"]
    assert "no geometry, grouping, layout schema" in inspected["reference_guidance"]
    assert {image["id"]: (image["width"], image["height"], image["png_hash"])
        for image in context["images"]} == {
            "household": (1810, 1416, "094ce20f169dfe41df903ad6fd76274d68d0831e2221eb23cb15684ef5308575"),
            "engineering-organisation": (1720, 1088, "1ce53b3d8bbee596adca8e0814c50c02341139c10f58d105aca546e77f549919")}
    for descriptor in context["images"]:
        result = await call("read_visualization_reference", {
            "request_key": job["key"], "reference_id": descriptor["id"]})
        metadata, image = result["content_blocks"]
        assert metadata["type"] == "text"
        metadata = json.loads(metadata["text"])
        assert metadata == {**descriptor, "request_key": job["key"],
            "source_reference_hash": job["reference_hash"],
            "evidence_scope": "design-reference-only; not a captured frame of the proposed scene"}
        assert image["type"] == "image" and image["media_type"] == "image/png"
        assert hashlib.sha256(base64.b64decode(image["data"], validate=True)).hexdigest() == descriptor["png_hash"]
    delivery = {"observation": "host_returned_tool_content", "request_key": job["key"],
        "source_reference_hash": job["reference_hash"],
        "images": [{"id": image["id"], "png_hash": image["png_hash"]} for image in context["images"]]}
    assert service.pending == {**before, "author_context_read": True, "author_reference_delivery": delivery}
    assert service.saved_request(job["key"])["author_reference_delivery"] == delivery
    assert service.architect.starts == 0
    saved = (service.root / "pending.json").read_text()
    assert "png_base64" not in saved and '"data"' not in saved


@pytest.mark.asyncio
async def test_on_demand_examples_remain_exact_and_do_not_replace_required_context_and_images(service):
    service._load()
    job = active_job(service)
    binding = service.request_binding(job["blueprint"], job["model"])
    await service.inspect({})
    assert "examples_read" not in job
    blocked = await service.publish({"request_key": job["key"], "code": CODE, "summary": "Offline program body."})
    assert blocked["valid"] is False
    examples = await service.read_examples({})
    assert set(examples) == {"request_key", "examples", "use"}
    assert set(examples["examples"]) == {"household", "organisation", "renderer"}
    assert "png_base64" not in json.dumps(examples)
    assert job["examples_read"] is True
    assert "author_reference_delivery" not in job
    assert (await service.publish({"request_key": job["key"], "code": CODE,
        "summary": "Examples do not deliver original pixels."}))["valid"] is False
    await service.read_reference({"request_key": job["key"], "reference_id": "household"})
    assert (await service.publish({"request_key": job["key"], "code": CODE,
        "summary": "One original image is insufficient."}))["valid"] is False
    await service.read_reference({"request_key": job["key"], "reference_id": "engineering-organisation"})
    assert (await service.publish({"request_key": job["key"], "code": CODE,
        "summary": "Offline program body."}))["valid"] is True
    receipt = job["publication_receipt"]
    assert receipt["request_key"] == binding["native_request_key"]
    assert receipt["reference_hash"] == binding["reference_hash"]
    assert service.request_binding(job["blueprint"], job["model"]) == binding
    assert set(receipt) == {"request_key", "code_hash", "blueprint_hash", "model_hash",
                           "reference_hash", "accepted_at", "authority"}
    assert "reference_context" not in json.loads((service.root / "pending.json").read_text())


@pytest.mark.asyncio
async def test_new_author_reference_guidance_is_request_local_with_no_persistent_profile_migration(service, monkeypatch):
    assert module.AUTHOR_REFERENCE_GUIDANCE not in module.VISUALIZER_PROMPT
    assert module.VISUALIZER_PROMPT.endswith(module.COMPOSITION_GUIDANCE)
    calls = native_completion(service, monkeypatch)
    from launchkit.architect import _native_request
    inspections = []

    async def request(client, method, url, **kwargs):
        if method == "post":
            inspections.append(await service.inspect({"request_key": service.pending["key"]}))
        return await _native_request(client, method, url, **kwargs)

    monkeypatch.setattr("launchkit.architect._native_request", request)
    await service.generate(*inputs())
    posts = [call for call in calls if call[0] == "post"]
    assert len(posts) == 1
    content = posts[0][2]["json"]["content"]
    assert module.AUTHOR_REFERENCE_GUIDANCE not in content
    assert "reference_guidance" in content and "include_author_references:true" in content
    assert "unchanged JavaScript examples and original renderer are optional" in content
    assert "both original images before composing" in content
    assert inspections[0]["reference_guidance"] == module.AUTHOR_REFERENCE_GUIDANCE


@pytest.mark.asyncio
@pytest.mark.parametrize("args", [None, [], {}, {"request_key": "foreign", "reference_id": "household"},
    {"reference_id": "household"}, {"request_key": None, "reference_id": "household"},
    {"request_key": "active", "reference_id": []}, {"request_key": "active", "reference_id": {}},
    {"request_key": "active", "reference_id": "unknown"},
    {"request_key": "active", "reference_id": "household", "extra": True}])
async def test_invalid_or_foreign_reference_reads_return_no_image_and_do_not_load_pixels(service, monkeypatch, args):
    job = active_job(service)
    if isinstance(args, dict) and args.get("request_key") == "active":
        args = {**args, "request_key": job["key"]}

    def unexpected(*_):
        raise AssertionError("Rejected request must not read reference pixels.")

    monkeypatch.setattr(references, "bundled_review_reference_context", unexpected)
    assert (await service.read_reference(args))["available"] is False
    assert not service.root.exists()


@pytest.mark.asyncio
async def test_no_active_request_and_source_hash_mismatch_never_expose_reference_content(service):
    assert (await service.read_reference({"request_key": "absent", "reference_id": "household"}))["available"] is False
    assert (await service.inspect({}))["available"] is False
    job = active_job(service)
    job["reference_hash"] = "0" * 64
    inspected = await service.inspect({})
    assert inspected["reference_context"]["available"] is False
    assert inspected["reference_context"]["images"] == []
    assert (await service.read_reference({"request_key": job["key"], "reference_id": "household"}))["available"] is False
    assert service.architect.starts == 0


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["clear", "replace-same-key", "replace-other-key", "mutate-key", "mutate-source"])
@pytest.mark.parametrize("callback", ["inspect", "read_reference"])
async def test_delayed_reference_read_cannot_cross_active_request_ownership(service, monkeypatch, change, callback):
    job = active_job(service)
    loaded = references.bundled_review_reference_context()
    started, release = threading.Event(), threading.Event()

    def delayed(*_):
        started.set()
        if not release.wait(3):
            raise AssertionError("Offline delayed reference fixture was not released.")
        return deepcopy(loaded)

    monkeypatch.setattr(references, "bundled_review_reference_context", delayed)
    args = {} if callback == "inspect" else {"request_key": job["key"], "reference_id": "household"}
    task = asyncio.create_task(getattr(service, callback)(args))
    try:
        assert await asyncio.to_thread(started.wait, 30)
        if change == "clear": service.pending = None
        elif change == "replace-same-key": service.pending = deepcopy(job)
        elif change == "replace-other-key": service.pending = {**deepcopy(job), "key": "1" * 64}
        elif change == "mutate-key": job["key"] = "1" * 64
        else: job["reference_hash"] = "2" * 64
    finally:
        release.set()
    result = await task
    if callback == "inspect" and change.startswith("mutate"):
        assert result["reference_context"]["available"] is False
        assert result["reference_context"]["images"] == []
    else:
        assert result["available"] is False
    assert "png_base64" not in json.dumps(result)
    assert service.architect.starts == 0
    assert not service.root.exists()


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["missing", "pixels", "source", "manifest", "symlink", "oversize", "recursive"])
async def test_unavailable_or_changed_reference_bundle_is_controlled_without_fallback_pixels(service, monkeypatch, tmp_path, change):
    original_root = module.REFERENCE_ROOT
    reference_root = tmp_path / "references"
    reference_root.mkdir()
    for name in ("review-images.json", "household.js", "scenes.js", "net.js", "household-reference.png", "engineering-reference.png"):
        shutil.copyfile(original_root / name, reference_root / name)
    job = active_job(service)
    if change == "missing": (reference_root / "household-reference.png").unlink()
    elif change == "pixels": (reference_root / "household-reference.png").write_bytes(b"invalid PNG")
    elif change == "source": (reference_root / "net.js").write_text("Changed renderer source.")
    elif change == "symlink":
        (reference_root / "household-reference.png").unlink()
        (reference_root / "household-reference.png").symlink_to(original_root / "household-reference.png")
    elif change == "oversize": (reference_root / "review-images.json").write_bytes(b" " * 16001)
    elif change == "recursive": (reference_root / "review-images.json").write_text("[" * 10000)
    else:
        manifest = json.loads((reference_root / "review-images.json").read_text())
        manifest["source_reference_hash"] = "0" * 64
        (reference_root / "review-images.json").write_text(json.dumps(manifest))
    monkeypatch.setattr(module, "REFERENCE_ROOT", reference_root)
    inspected = await service.inspect({})
    assert inspected["reference_context"]["available"] is False
    assert inspected["reference_context"]["images"] == []
    result = await service.read_reference({"request_key": job["key"], "reference_id": "household"})
    assert result["available"] is False
    assert str(reference_root) not in json.dumps(result)
    assert "png_base64" not in json.dumps(result)
    assert service.architect.starts == 0


@pytest.mark.asyncio
@pytest.mark.parametrize("reference_id,filename", [
    ("household", "household-reference.png"),
    ("engineering-organisation", "engineering-reference.png"),
])
async def test_checked_image_only_change_invalidates_current_cache_but_preserves_history(
        service, monkeypatch, tmp_path, reference_id, filename):
    from test_visualization_review import png

    root = tmp_path / "references"
    shutil.copytree(module.REFERENCE_ROOT, root)
    monkeypatch.setattr(module, "REFERENCE_ROOT", root)
    examples = module.visualization_examples()
    blueprint, model = inputs()
    original_binding = service.request_binding(blueprint, model)
    calls = native_completion(service, monkeypatch)
    original = await service.generate(blueprint, model)
    original_path = service.root / f"{original['key']}.json"
    original_bytes = original_path.read_bytes()
    assert (await service.generate(blueprint, model))["cached"] is True

    manifest_path = root / "review-images.json"
    manifest = json.loads(manifest_path.read_text())
    manifest["images"].reverse()
    manifest_path.write_text(json.dumps(manifest))
    assert service.request_binding(blueprint, model) == original_binding
    assert (await service.generate(blueprint, model))["cached"] is True

    data = png(20, 20)
    (root / filename).write_bytes(data)
    changed = next(image for image in manifest["images"] if image["id"] == reference_id)
    changed.update(width=20, height=20, png_hash=hashlib.sha256(data).hexdigest())
    manifest_path.write_text(json.dumps(manifest))
    assert module.visualization_examples() == examples
    current = service.request_binding(blueprint, model)
    assert current["reference_hash"] != original_binding["reference_hash"]
    assert current["native_request_key"] != original["key"]
    assert current["blueprint_hash"] == original["blueprint_hash"]
    assert current["model_hash"] == original["model_hash"]
    assert references.bundled_review_reference_context(root)["source_reference_hash"] == current["reference_hash"]

    service.pending = {**{key: value for key, value in original_binding.items() if key != "native_request_key"},
        "key": original["key"], "blueprint": blueprint, "model": model}
    stale = await service.inspect({"request_key": original["key"], "include_author_references": True})
    assert stale["available"] is False and "author_reference_delivery" not in service.pending
    service.pending = None

    updated = await service.generate(blueprint, model)
    assert updated["key"] == current["native_request_key"] and not updated["cached"]
    assert len([call for call in calls if call[0] == "post"]) == 2
    assert (await service.generate(blueprint, model))["cached"] is True
    assert len([call for call in calls if call[0] == "post"]) == 2
    assert original_path.read_bytes() == original_bytes
    assert service._cache(original["key"])["key"] == original["key"]
    assert service.saved_request(original["key"])["reference_hash"] == original["reference_hash"]


@pytest.mark.asyncio
async def test_missing_checked_image_changes_identity_and_blocks_new_publication(service, monkeypatch, tmp_path):
    root = tmp_path / "references"
    shutil.copytree(module.REFERENCE_ROOT, root)
    monkeypatch.setattr(module, "REFERENCE_ROOT", root)
    blueprint, model = inputs()
    checked = service.request_binding(blueprint, model)
    (root / "household-reference.png").unlink()
    text_only = service.request_binding(blueprint, model)
    assert text_only["reference_hash"] != checked["reference_hash"]
    assert text_only["reference_hash"] == module.reference_content_hash(fingerprint(module.visualization_examples()), [])
    job = active_job(service)
    assert job["key"] == text_only["native_request_key"]
    bundle = await service.inspect({"view": "overview", "request_key": job["key"], "include_author_references": True})
    assert bundle["available"] is False
    await service.inspect({})
    await service.read_examples({})
    assert job["examples_read"] is True and "author_reference_delivery" not in job
    assert (await service.publish({"request_key": job["key"], "code": CODE,
        "summary": "Missing original pixels."}))["valid"] is False
    assert not service._cache(job["key"]) and service.architect.starts == 0
