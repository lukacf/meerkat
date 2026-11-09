"""Offline evidence contracts; synthetic verdicts do not prove visual quality."""
import asyncio
import base64
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import re
import shutil

import pytest

from launchkit import visualization_review as module
from launchkit.projects import fingerprint
from meerkat_meta.agent_builder import CallbackDispatcher
from meerkat_meta.types import HelperResult
from test_visualization_review import Operations, Runtime, frame, png, report, service, source, finish


@pytest.fixture(scope="module")
def originals():
    return module.bundled_review_reference_context()


def context(originals):
    value = deepcopy(originals)
    for image in value["images"]:
        data = png(20, 20)
        image.update(width=20, height=20, png_hash=hashlib.sha256(data).hexdigest(),
            png_base64=base64.b64encode(data).decode())
    return value


def reference_source(operations, reference_context):
    body = source(operations)
    receipt = operations.receipts[body["binding"]["id"]]
    artifact = receipt["artifact"]
    body["binding"]["reference_hash"] = artifact["reference_hash"] = reference_context["source_reference_hash"]
    identity = fingerprint({key: artifact[key] for key in ("contract", "blueprint_hash", "model_hash", "reference_hash")})
    body["binding"]["native_request_key"] = receipt["native_request_key"] = artifact["key"] = identity
    artifact["publication_receipt"].update(reference_hash=reference_context["source_reference_hash"], request_key=identity)
    return body


class ReferenceRuntime(Runtime):
    async def spawn_helper(self, identity, task, **options):
        self.calls.append((identity, task, options))
        self.started.set()
        review_id = re.search(r"review_id is ([a-f0-9]{64})", task)[1]
        capability = re.search(r"capability is ([a-f0-9]{64})", task)[1]
        dispatcher = CallbackDispatcher()
        review_service = self.service

        class Builder:
            async def build_agent(self, draft):
                review_service.register_tools(draft)

        dispatcher.register_builder(Builder())
        scope = "fixture-reference-build-" + identity
        built = await dispatcher.handle_callback("callback/build_agent", {
            "options": {"scope_id": scope, "profile_name": module.REVIEW_PROFILE}})
        assert {item["name"] for item in built["tools"]} == {
            "inspect_visualization_review", "read_visualization_frame", "read_visualization_reference", "publish_visualization_review"}
        publication = next(tool for tool in built["tools"] if tool["name"] == "publish_visualization_review")
        assert "reference_receipts" in publication["input_schema"]["required"]

        async def call(name, args):
            return await dispatcher.handle_callback("callback/call_tool", {"scope_id": scope, "tool": name, "arguments": args})

        inspected = (await call("inspect_visualization_review", {"review_id": review_id, "capability": capability}))["content"]
        assert "png_base64" not in json.dumps(inspected)
        frame_receipts, reference_receipts = {}, {}
        for item in inspected["frames"]:
            result = await call("read_visualization_frame", {"review_id": review_id, "frame_id": item["id"]})
            frame_receipts[item["id"]] = json.loads(result["content_blocks"][0]["text"])["frame_receipt"]
        for item in inspected["reference_context"]["images"]:
            result = await call("read_visualization_reference", {"review_id": review_id, "reference_id": item["id"]})
            metadata, image = result["content_blocks"]
            metadata = json.loads(metadata["text"])
            assert metadata["origin"] == "user-supplied-design-reference"
            assert "not a captured frame" in metadata["evidence_scope"]
            assert image["type"] == "image" and image["media_type"] == "image/png"
            assert hashlib.sha256(base64.b64decode(image["data"])).hexdigest() == item["png_hash"]
            reference_receipts[item["id"]] = metadata["reference_receipt"]
            self.images.append(image)
        value = report(review_id, frame_receipts)
        value["reference_receipts"] = reference_receipts
        assert (await call("publish_visualization_review", value))["content"]["valid"]
        return HelperResult(output="Fixture publication only.", tokens_used=7, session_id="fixture-reference-session")


def test_bundled_images_preserve_exact_supplied_originals_and_source_hashes(originals):
    assert originals["evidence_scope"] == "user-supplied-design-reference-pixels"
    assert originals["source_reference_hash"] == "04660b93fdc73fa938ba0439ebd3f65f91c36331bb3aeb2038098fe4166a14b7"
    assert {image["id"]: (image["width"], image["height"], image["png_hash"])
        for image in originals["images"]} == {
            "household": (1810, 1416, "094ce20f169dfe41df903ad6fd76274d68d0831e2221eb23cb15684ef5308575"),
            "engineering-organisation": (1720, 1088, "1ce53b3d8bbee596adca8e0814c50c02341139c10f58d105aca546e77f549919")}
    for image in originals["images"]:
        assert hashlib.sha256(base64.b64decode(image["png_base64"])).hexdigest() == image["png_hash"]
    assert "not captured reference" not in originals["brief"]
    assert "png_base64" not in json.dumps(module._reference_manifest(originals))


@pytest.mark.asyncio
async def test_real_installed_dispatcher_returns_separate_reference_images_and_persists_exposure(tmp_path, originals):
    operations = Operations()
    value = deepcopy(originals)
    runtime = ReferenceRuntime()
    review, _ = service(operations, runtime=runtime, root=tmp_path, reference_context=value)
    receipt = await review.submit(**reference_source(operations, value))
    value["images"][0]["png_base64"] = "later caller mutation"
    await finish(review)
    result = review.get(receipt["id"])
    assert result["status"] == "reviewed" and len(runtime.images) == 2
    assert len(result["frames"]) == 1
    assert "png_base64" not in json.dumps(result)
    saved = json.loads((tmp_path / (receipt["id"] + ".json")).read_text())
    assert set(saved["report_binding"]["reference_receipt_hashes"]) == {"household", "engineering-organisation"}
    assert saved["snapshot"]["presentation_scope"] == module.PRESENTATION_SCOPE
    restored, other = service(operations, root=tmp_path)
    assert restored.get(receipt["id"])["report"] == result["report"]
    assert not other.calls


@pytest.mark.asyncio
async def test_known_legacy_publication_gets_checked_pixels_without_rewriting_history(tmp_path, originals):
    operations = Operations()
    legacy = deepcopy(originals)
    legacy["source_reference_hash"] = fingerprint(module.visualization_examples())
    body = reference_source(operations, legacy)
    publications = deepcopy(operations.receipts)
    old_review, _ = service(operations, root=tmp_path)
    old = await old_review.submit(**body)
    await finish(old_review)
    old_path = tmp_path / (old["id"] + ".json")
    old_bytes = old_path.read_bytes()

    runtime = ReferenceRuntime()
    review, _ = service(operations, runtime=runtime, root=tmp_path, reference_context=originals)
    submitted = await review.submit(**body)
    await finish(review)
    assert submitted["id"] != old["id"]
    assert review.get(submitted["id"])["status"] == "reviewed"
    path = tmp_path / (submitted["id"] + ".json")
    new_bytes = path.read_bytes()
    snapshot = json.loads(new_bytes)["snapshot"]
    assert snapshot["binding"] == body["binding"]
    assert snapshot["reference_context"] == legacy
    assert review.reference_context == originals
    assert originals["source_reference_hash"] == module.reference_content_hash(
        legacy["source_reference_hash"], originals["images"])
    assert operations.receipts == publications and old_path.read_bytes() == old_bytes
    restored, fresh_runtime = service(operations, root=tmp_path, reference_context=originals)
    assert restored.get(old["id"])["report"] == old_review.get(old["id"])["report"]
    assert restored.get(submitted["id"])["report"] == review.get(submitted["id"])["report"]
    assert not fresh_runtime.calls
    assert old_path.read_bytes() == old_bytes and path.read_bytes() == new_bytes


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["unknown-publication-source", "changed-source-examples",
    "different-source-bundle", "stale-pixel-composite", "invalid-pixels"])
async def test_legacy_review_rejects_unmatched_source_or_pixel_bundle(tmp_path, originals, monkeypatch, change):
    operations = Operations()
    legacy = deepcopy(originals)
    legacy["source_reference_hash"] = fingerprint(module.visualization_examples())
    if change == "unknown-publication-source":
        legacy["source_reference_hash"] = "0" * 64
    body = reference_source(operations, legacy)
    value = deepcopy(originals)
    if change == "changed-source-examples":
        examples = module.visualization_examples()
        examples["household"] += "\n// Changed source example."
        monkeypatch.setattr(module, "visualization_examples", lambda: examples)
    elif change == "different-source-bundle":
        value["source_reference_hash"] = module.reference_content_hash("0" * 64, value["images"])
    elif change == "stale-pixel-composite":
        value = context(originals)
    elif change == "invalid-pixels":
        value["images"][0]["png_hash"] = "0" * 64
    review, runtime = service(operations, root=tmp_path, reference_context=value)
    publications = deepcopy(operations.receipts)
    with pytest.raises(ValueError):
        await review.submit(**body)
    assert not runtime.calls and not review.tasks and not list(tmp_path.iterdir())
    assert operations.receipts == publications and review.reference_context == value


@pytest.mark.asyncio
async def test_reference_receipts_and_owner_are_required_without_becoming_scene_findings(originals):
    operations = Operations()
    value = context(originals)
    review, runtime = service(operations, reference_context=value)
    runtime.release = asyncio.Event()
    receipt = await review.submit(**reference_source(operations, value))
    callbacks = module.ScopedReviewCallbacks(review)
    await callbacks.inspect_visualization_review({"review_id": receipt["id"], "capability": review.receipts[receipt["id"]]["capability"]})
    with pytest.raises(ValueError, match="does not own"):
        await module.ScopedReviewCallbacks(review).read_visualization_reference({"review_id": receipt["id"], "reference_id": "household"})
    with pytest.raises(ValueError, match="frame is unavailable"):
        await callbacks.read_visualization_frame({"review_id": receipt["id"], "frame_id": "household"})
    with pytest.raises(ValueError, match="reference image is unavailable"):
        await callbacks.read_visualization_reference({"review_id": receipt["id"], "reference_id": "frame-one"})
    scene = await callbacks.read_visualization_frame({"review_id": receipt["id"], "frame_id": "frame-one"})
    token = json.loads(scene.blocks[0]["text"])["frame_receipt"]
    args = report(receipt["id"], {"frame-one": token})
    with pytest.raises(ValueError, match="every original"):
        await callbacks.publish_visualization_review(args)
    receipts = {}
    for image in value["images"]:
        result = await callbacks.read_visualization_reference({"review_id": receipt["id"], "reference_id": image["id"]})
        receipts[image["id"]] = json.loads(result.blocks[0]["text"])["reference_receipt"]
    args["reference_receipts"] = {key: "0" * 32 for key in receipts}
    with pytest.raises(ValueError, match="every original"):
        await callbacks.publish_visualization_review(args)
    args["reference_receipts"] = receipts
    invented = deepcopy(args)
    invented["findings"][0]["frame_id"] = "household"
    with pytest.raises(ValueError, match="observed frame"):
        await callbacks.publish_visualization_review(invented)
    assert (await callbacks.publish_visualization_review(args))["valid"]
    await review.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["pixels", "origin", "dimensions", "source", "brief", "duplicate"])
async def test_invalid_reference_pixels_or_provenance_rejected_before_admission(tmp_path, originals, change):
    operations = Operations()
    value = context(originals)
    body = reference_source(operations, value)
    if change == "pixels": value["images"][0]["png_hash"] = "0" * 64
    elif change == "origin": value["images"][0]["origin"] = "current-rendered-scene"
    elif change == "dimensions": value["images"][0]["width"] += 1
    elif change == "source": value["source_reference_hash"] = "0" * 64
    elif change == "brief": value["brief"] += " changed"
    else: value["images"][1]["id"] = value["images"][0]["id"]
    review, runtime = service(operations, root=tmp_path, reference_context=value)
    with pytest.raises(ValueError):
        await review.submit(**body)
    assert not runtime.calls and not review.tasks and not list(tmp_path.iterdir())


@pytest.mark.asyncio
@pytest.mark.parametrize("field", ["width", "height", "pixel_ratio", "selected_id", "show_tools", "playing", "reduced_motion"])
async def test_mixed_presentations_are_rejected_before_admission(tmp_path, field):
    operations = Operations()
    body = source(operations)
    first, second = frame("first"), frame("second")
    first.update(playing=True, reduced_motion=False, time=1)
    second.update(playing=True, reduced_motion=False, time=10)
    if field in {"width", "height", "pixel_ratio"}:
        geometry = {"width": 390, "height": 300, "ratio": 1}
        geometry[{"pixel_ratio": "ratio"}.get(field, field)] += 1
        second = frame("second", **geometry)
        second.update(playing=True, reduced_motion=False, time=10)
    elif field == "selected_id": second[field] = body["model"]["nodes"][0]["id"]
    else: second[field] = not second[field]
    body["frames"] = [first, second]
    review, runtime = service(operations, root=tmp_path)
    with pytest.raises(ValueError, match="one playing presentation"):
        await review.submit(**body)
    assert not runtime.calls and not review.tasks and not list(tmp_path.iterdir())


def test_same_playing_view_and_single_static_view_remain_valid():
    operations = Operations()
    body = source(operations)
    module.checked_submission(operations, **body)
    body["frames"] = [frame("first"), frame("second")]
    with pytest.raises(ValueError, match="playing presentation"):
        module.checked_submission(operations, **body)
    for index, captured in enumerate(body["frames"]):
        captured.update(playing=True, reduced_motion=False, time=index * 15,
            runtime_activity={"sequence": index})
    snapshot, _ = module.checked_submission(operations, **body)
    assert snapshot["presentation_scope"] == module.PRESENTATION_SCOPE


@pytest.mark.asyncio
async def test_legacy_mixed_view_receipt_remains_exactly_loadable_without_respawn(tmp_path):
    operations = Operations()
    body = source(operations)
    review, runtime = service(operations, root=tmp_path)
    submitted = await review.submit(**body)
    await finish(review)
    saved = json.loads((tmp_path / (submitted["id"] + ".json")).read_text())
    legacy_body = deepcopy(body)
    legacy_body["frames"].append(frame("historic-second-view", width=450))
    snapshot, identity = module.checked_submission(operations, **legacy_body, require_single_presentation=False)
    saved.update(id=identity, snapshot=snapshot)
    saved["report_binding"].update(review_id=identity)
    saved["report_binding"]["frame_receipt_hashes"]["historic-second-view"] = fingerprint("historic-result")
    path = tmp_path / (identity + ".json")
    path.write_text(json.dumps(saved))
    restored, fresh_runtime = service(operations, root=tmp_path)
    assert restored.get(identity)["report"] == saved["report"]
    assert "presentation_scope" not in restored.receipts[identity]["snapshot"]
    assert "reference_receipt_hashes" not in restored.receipts[identity]["report_binding"]
    assert not fresh_runtime.calls


@pytest.mark.parametrize("change", ["image", "source", "manifest", "path", "symlink", "missing-name", "duplicate"])
def test_bundle_loader_refuses_changed_originals_sources_and_paths(tmp_path, change):
    original = module.REFERENCE_ROOT
    for name in ("review-images.json", "household.js", "scenes.js", "net.js", "household-reference.png", "engineering-reference.png"):
        shutil.copyfile(original / name, tmp_path / name)
    if change == "image": (tmp_path / "household-reference.png").write_bytes(png(20, 20))
    elif change == "source": (tmp_path / "household.js").write_text("changed")
    elif change == "symlink":
        (tmp_path / "household-reference.png").unlink()
        (tmp_path / "household-reference.png").symlink_to(original / "household-reference.png")
    else:
        manifest = json.loads((tmp_path / "review-images.json").read_text())
        if change == "manifest": manifest["source_reference_hash"] = "0" * 64
        elif change == "path": manifest["images"][0]["filename"] = "../household-reference.png"
        elif change == "missing-name":
            manifest["images"][0].pop("id")
            manifest["images"][0].pop("filename")
        else: manifest["images"][1] = deepcopy(manifest["images"][0])
        (tmp_path / "review-images.json").write_text(json.dumps(manifest))
    with pytest.raises(ValueError):
        module.bundled_review_reference_context(tmp_path)


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["reference-pixels", "reference-exposure", "presentation-scope"])
async def test_retained_pixel_receipt_rejects_changed_evidence_without_respawn(tmp_path, originals, change):
    operations = Operations()
    value = context(originals)
    runtime = ReferenceRuntime()
    review, _ = service(operations, runtime=runtime, root=tmp_path, reference_context=value)
    receipt = await review.submit(**reference_source(operations, value))
    await finish(review)
    path = tmp_path / (receipt["id"] + ".json")
    saved = json.loads(path.read_text())
    if change == "reference-pixels": saved["snapshot"]["reference_context"]["images"][0]["png_hash"] = "0" * 64
    elif change == "reference-exposure": saved["report_binding"]["reference_receipt_hashes"] = {}
    else: saved["snapshot"]["presentation_scope"] = "invented-scope"
    path.write_text(json.dumps(saved))
    restored, fresh_runtime = service(operations, root=tmp_path)
    with pytest.raises(ValueError):
        restored.get(receipt["id"])
    assert not fresh_runtime.calls


def test_frozen_reference_batch_is_bounded(originals, monkeypatch):
    operations = Operations()
    value = context(originals)
    body = reference_source(operations, value)
    monkeypatch.setattr(module, "MAX_REFERENCE_BYTES", 1)
    with pytest.raises(ValueError, match="reference pixels changed"):
        module.checked_submission(operations, **body, reference_context=value)
