"""Offline contract tests. Fake critic verdicts do not establish artistic quality."""
import asyncio
import base64
from copy import deepcopy
import hashlib
import importlib.util
import json
import math
from pathlib import Path
import re
import struct
import sys
import time
from types import SimpleNamespace
import zlib

import pytest

from launchkit import visualization_review as module
MAX_PNG_BYTES, REVIEW_PROFILE = module.MAX_PNG_BYTES, module.REVIEW_PROFILE
ReviewAdmissionUnavailable, ScopedReviewCallbacks = module.ReviewAdmissionUnavailable, module.ScopedReviewCallbacks
VisualizationReviewService = module.VisualizationReviewService
_advisory_feedback, checked_submission = module._advisory_feedback, module.checked_submission
from launchkit.blueprint import starter_blueprint
from launchkit.projects import fingerprint
from meerkat_meta.agent_builder import CallbackDispatcher
from meerkat_meta.types import HelperResult

RENDERER = fingerprint({"renderer": "fixture"})


def png(width=390, height=300):
    def chunk(kind, value):
        return struct.pack(">I", len(value)) + kind + value + struct.pack(">I", zlib.crc32(kind + value) & 0xffffffff)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress((b"\0" + b"\xff\xff\xff\xff" * width) * height)) + chunk(b"IEND", b""))


def frame(identity="frame-one", width=390, height=300, ratio=1):
    data = png(math.floor(width * ratio + .5), math.floor(height * ratio + .5))
    return {"id": identity, "width": width, "height": height, "pixel_ratio": ratio, "time": 0,
            "selected_id": None, "show_tools": False, "reduced_motion": True, "playing": False,
            "progress": 0, "runtime_activity": None, "png_base64": base64.b64encode(data).decode(),
            "png_hash": hashlib.sha256(data).hexdigest()}


class Operations:
    def __init__(self):
        self.receipts, self.streams = {}, {}
        self.mutations = []

    def get(self, identity):
        return self.receipts.get(identity)

    def _desired_head(self, scope):
        return self.streams.get(scope)

    def _correction_reservation(self, identity):
        return None

    async def submit(self, *args, **kwargs):
        self.mutations.append((args, kwargs))
        raise AssertionError("Review cannot submit a correction.")


def source(operations, version="A"):
    blueprint = starter_blueprint().model_dump(mode="json")
    blueprint["project"]["name"] = "Frozen proposal " + version
    model = {"nodes": [{"id": identity, "kind": "agent", "name": identity} for identity in starter_blueprint().identities()], "edges": [], "phases": []}
    native = {"contract": "freeform-animation-v3", "blueprint_hash": fingerprint(blueprint),
              "model_hash": fingerprint(model), "reference_hash": fingerprint({"references": "source"})}
    key = fingerprint(native)
    code = "return {duration:36,render(frame){frame.ctx.clearRect(0,0,frame.width,frame.height);}};"
    code_hash = hashlib.sha256(code.encode()).hexdigest()
    identity = fingerprint({"operation": version})
    receipt = {"id": identity, "setup_id": "setup", "stream_id": "overview", "source_generation": 1,
               "request_fingerprint": fingerprint({"blueprint": blueprint, "model": model}),
               "native_request_key": key, "blueprint_hash": native["blueprint_hash"], "model_hash": native["model_hash"],
               "revision_binding": None, "artifact": {**native, "key": key, "code": code, "code_hash": code_hash,
                  "publication_receipt": {**native, "authority": "publish_visualization", "request_key": key, "code_hash": code_hash}}}
    operations.receipts[identity] = receipt
    operations.streams[("setup", "overview")] = identity
    binding = {name: receipt.get(name) for name in ("id", "setup_id", "stream_id", "source_generation", "request_fingerprint", "native_request_key", "blueprint_hash", "model_hash", "revision_binding")}
    binding.update(code_hash=code_hash, reference_hash=native["reference_hash"])
    return {"binding": binding, "blueprint": blueprint, "model": model, "frames": [frame()], "renderer_hash": RENDERER}


def report(review_id, receipts, *, verdict="needs_revision"):
    return {"review_id": review_id, "evidence_hash": review_id, "frame_receipts": receipts,
            "verdict": verdict, "summary": "The fixture scene leaves the configured team invisible.",
            "findings": [{"frame_id": "frame-one", "kind": "problem" if verdict == "needs_revision" else "strength",
                          "observation": "Only the empty background is visible in this frame.",
                          "repair_hint": "Make the current configured team visible.", "source_paths": ["/model/nodes"]}]}


class Runtime:
    def __init__(self):
        self.calls, self.images = [], []
        self.started, self.release = asyncio.Event(), None
        self.raise_after_publication = False
        self.publish = True
        self.before_publication = None
        self.service = None

    def mob_handle(self):
        return self

    async def spawn_helper(self, identity, task, **options):
        self.calls.append((identity, task, options))
        self.started.set()
        if self.release is not None:
            await self.release.wait()
        review_id = re.search(r"review_id is ([a-f0-9]{64})", task)[1]
        capability = re.search(r"capability is ([a-f0-9]{64})", task)[1]
        dispatcher = CallbackDispatcher()
        service = self.service
        class Builder:
            async def build_agent(self, draft):
                assert draft.profile_name == REVIEW_PROFILE
                service.register_tools(draft)
        dispatcher.register_builder(Builder())
        scope = "fixture-native-build-" + identity
        built = await dispatcher.handle_callback("callback/build_agent", {"options": {"scope_id": scope, "profile_name": REVIEW_PROFILE}})
        assert {item["name"] for item in built["tools"]} == {"inspect_visualization_review", "read_visualization_frame", "publish_visualization_review"}
        async def call(tool, arguments):
            return await dispatcher.handle_callback("callback/call_tool", {"scope_id": scope, "tool": tool, "arguments": arguments})
        inspected = (await call("inspect_visualization_review", {"review_id": review_id, "capability": capability}))["content"]
        assert "code" not in inspected and "summary" not in inspected
        assert "reference_context" in inspected
        receipts = {}
        for item in inspected["frames"]:
            result = await call("read_visualization_frame", {"review_id": review_id, "frame_id": item["id"]})
            assert "content_blocks" in result and "content" not in result
            metadata, image = result["content_blocks"]
            assert image["type"] == "image" and image["source"] == "inline" and image["media_type"] == "image/png"
            assert hashlib.sha256(base64.b64decode(image["data"])).hexdigest() == item["png_hash"]
            self.images.append(image)
            receipts[item["id"]] = json.loads(metadata["text"])["frame_receipt"]
        if self.before_publication is not None:
            await self.before_publication(call, review_id, receipts)
        if self.publish:
            assert (await call("publish_visualization_review", report(review_id, receipts)))["content"]["valid"]
        if self.raise_after_publication:
            raise OSError("Simulated lost helper response.")
        return HelperResult(output="Returned narration is not review authority.", tokens_used=7, session_id="fixture-session")


class TransportTimedOut:
    """The native request is sent, then its transport times out: its outcome stays unknown."""

    async def wait(self):
        raise TimeoutError("inert transport timeout specimen")


def service(operations, runtime=None, root=None, **options):
    runtime = runtime or Runtime()
    result = VisualizationReviewService(runtime=runtime, operations=operations, root=root, expected_renderer_hash=RENDERER, **options)
    if isinstance(runtime, Runtime):
        runtime.service = result
    return result, runtime


async def finish(service):
    await asyncio.gather(*list(service.tasks.values()))


@pytest.mark.asyncio
async def test_installed_dispatcher_delivers_actual_image_and_source_bound_advisory(tmp_path):
    operations = Operations()
    body = source(operations)
    before = deepcopy(body)
    review, runtime = service(operations, root=tmp_path)
    receipt = await review.submit(**body)
    body["blueprint"]["project"]["name"] = "Later caller mutation"
    body["frames"][0]["time"] = 50
    await finish(review)
    result = review.get(receipt["id"])
    assert result["status"] == "reviewed" and result["report"]["verdict"] == "needs_revision"
    assert runtime.images and len(runtime.calls) == 1
    options = dict(runtime.calls[0][2])
    assert 0 < options.pop("transport_timeout") <= review.timeout
    assert options == {"result_label": "visual-review-result", "max_text_bytes": 16000, "role": "visual-review", "runtime_mode": "turn_driven"}
    assert result["correction_advisory"]["expected_parent"] == {"operation_id": before["binding"]["id"], **{key: before["binding"][key] for key in ("code_hash", "blueprint_hash", "model_hash", "setup_id", "stream_id")}}
    assert result["correction_advisory"]["eligible_now"] is True and not operations.mutations
    assert review.receipts[receipt["id"]]["snapshot"]["blueprint"] == before["blueprint"]
    assert "capability" not in result and "png_base64" not in result["frames"][0]
    assert (tmp_path / (receipt["id"] + ".json")).stat().st_mode & 0o777 == 0o600


@pytest.mark.asyncio
@pytest.mark.parametrize("diagnostic_import_available,exception_kind,boundary", [
    (True, "transport", "helper_await"),
    (False, "transport", "helper_await"),
    (True, "value_error", "helper_await"),
    (True, "key_error", "helper_await"),
    (True, "type_error", "helper_await"),
    (True, "value_error", "result_fields"),
    (True, "key_error", "result_fields"),
    (True, "type_error", "result_fields"),
])
async def test_duplicate_submission_and_lost_response_preserve_checked_publication(
        tmp_path, monkeypatch, diagnostic_import_available, exception_kind, boundary):
    operations = Operations()
    body = source(operations)
    review, runtime = service(operations, root=tmp_path)
    runtime.release = asyncio.Event()
    sensitive_detail = "synthetic-private-password-do-not-retain"
    sensitive_type = "SyntheticPrivateDiagnosticType"
    if exception_kind != "transport":
        exception_class = {"value_error": ValueError, "key_error": KeyError,
            "type_error": TypeError}[exception_kind]
        original_spawn = runtime.spawn_helper
        error = type(sensitive_type, (exception_class,), {})(sensitive_detail)

        async def validation_failure(identity, task, **options):
            await original_spawn(identity, task, **options)
            if boundary == "helper_await":
                raise error

            class ReturnedResult:
                @property
                def session_id(self):
                    raise error

            return ReturnedResult()

        monkeypatch.setattr(runtime, "spawn_helper", validation_failure)
    first = await review.submit(**body)
    await runtime.started.wait()
    assert (await review.submit(**body))["id"] == first["id"] and len(runtime.calls) == 1
    blocked_imports = []
    if not diagnostic_import_available:
        import builtins
        original_import = builtins.__import__

        def import_without_diagnostic_classes(name, globals=None, locals=None, fromlist=(), level=0):
            if name == "meerkat_meta.errors" and set(fromlist or ()) == {"RpcError", "TransportError"}:
                blocked_imports.append(True)
                raise ImportError("Synthetic private diagnostic import detail.")
            return original_import(name, globals, locals, fromlist, level)

        monkeypatch.setattr(builtins, "__import__", import_without_diagnostic_classes)
    runtime.raise_after_publication = exception_kind == "transport"
    runtime.release.set()
    await finish(review)
    result = review.get(first["id"])
    assert result["status"] == "reviewed" and result["helper_state"] == "result_unknown"
    saved = json.loads((tmp_path / (first["id"] + ".json")).read_text())
    category = ("validation" if exception_kind != "transport" else
        "transport" if diagnostic_import_available else "unexpected")
    kind = exception_kind if diagnostic_import_available else "unexpected"
    assert saved["failure_diagnostic"] == {"category": category, "stage": "post_dispatch",
        "exception_kind": kind, "boundary": boundary}
    assert blocked_imports == ([] if diagnostic_import_available else [True])
    assert saved["dispatch_started"] is True and first["id"] in review.active_reviews
    assert review.publication_reviews[fingerprint(body["binding"])] == {first["id"]}
    assert "Simulated lost helper response" not in json.dumps(saved)
    assert "Synthetic private diagnostic import detail" not in json.dumps(saved)
    assert sensitive_detail not in json.dumps(saved) and sensitive_type not in json.dumps(saved)
    assert sensitive_detail not in json.dumps(result) and sensitive_type not in json.dumps(result)
    assert "failure_diagnostic" not in result
    restored, other_runtime = service(operations, root=tmp_path)
    assert (await restored.submit(**body))["report"] == result["report"]
    assert restored.receipts[first["id"]]["failure_diagnostic"] == saved["failure_diagnostic"]
    assert not other_runtime.calls and not restored.tasks


@pytest.mark.asyncio
async def test_abrupt_restart_unknown_attempt_never_respawns(tmp_path):
    operations = Operations()
    body = source(operations)
    review, runtime = service(operations, root=tmp_path)
    runtime.release = asyncio.Event()
    first = await review.submit(**body)
    await runtime.started.wait()
    restored, other_runtime = service(operations, root=tmp_path)
    retained = await restored.submit(**body)
    assert retained["status"] == "inconclusive" and retained["helper_state"] == "result_unknown"
    assert not other_runtime.calls and not restored.tasks
    await review.shutdown()


@pytest.mark.asyncio
async def test_old_A_review_finishes_after_B_without_admitting_correction():
    operations = Operations()
    body = source(operations)
    review, runtime = service(operations)
    runtime.release = asyncio.Event()
    first = await review.submit(**body)
    await runtime.started.wait()
    newer = source(operations, "B")
    runtime.release.set()
    await finish(review)
    result = review.get(first["id"])
    assert result["report"]["verdict"] == "needs_revision" and result["current"] is False
    assert result["correction_advisory"]["eligible_now"] is False
    assert result["correction_advisory"]["expected_parent"]["operation_id"] == body["binding"]["id"]
    assert operations._desired_head(("setup", "overview")) == newer["binding"]["id"]
    assert not operations.mutations


@pytest.mark.asyncio
@pytest.mark.parametrize("retirement", ["release", "expire", "replace"])
async def test_managed_review_uses_live_consumer_and_revocation_without_legacy_head(retirement):
    from launchkit.visualization_operations import VisualizationOperations
    from launchkit.visualization_consumers import INTENT_FIELDS

    now = [0]
    operations = VisualizationOperations(consumer_clock=lambda: now[0])
    body = source(operations)
    operation = operations.receipts[body["binding"]["id"]]
    # This fixture uses the pre-continuation publication contract; its real
    # operation receipt omits the optional revision member entirely.
    operation.pop("revision_binding")
    operation["consumer_managed"] = True
    operations.streams.clear()
    scope = {"consumer_id": "00000000-0000-0000-0000-000000000001",
             "intent_revision": 1, "setup_id": "setup", "stream_id": "overview"}
    declared = operations.consumers.declare(**scope)
    intent = {key: declared[key] for key in INTENT_FIELDS}
    operations.consumers.attach(intent, operation)
    assert operations._desired_head(("setup", "overview")) is None
    review, runtime = service(operations)
    first = await review.submit(**body)
    await finish(review)
    result = review.get(first["id"])
    assert result["status"] == "reviewed" and result["current"] is True
    assert result["correction_advisory"]["eligible_now"] is True
    saved_report = deepcopy(result["report"])

    if retirement == "release":
        operations.consumers.release(**intent)
    elif retirement == "expire":
        now[0] = 91
    else:
        operations.consumers.declare(**{**scope, "intent_revision": 2})
    retired = review.get(first["id"])
    assert retired["current"] is False
    assert retired["correction_advisory"]["eligible_now"] is False
    assert retired["report"] == saved_report and len(runtime.calls) == 1


@pytest.mark.asyncio
async def test_managed_review_unknown_consumer_projection_cannot_use_legacy_head():
    operations = Operations()
    body = source(operations)
    operation = operations.receipts[body["binding"]["id"]]
    operation.update(consumer_managed=True, scheduling={"managed_consumers": None, "desired": True})
    review, _ = service(operations)
    first = await review.submit(**body)
    await finish(review)
    result = review.get(first["id"])
    assert result["current"] is None
    assert result["correction_advisory"]["eligible_now"] is False


@pytest.mark.asyncio
async def test_spent_parent_correction_stays_ineligible_after_setup_switch_and_restart(tmp_path):
    from launchkit.visualization_operations import VisualizationOperations
    from launchkit.visualization_consumers import INTENT_FIELDS

    operations = VisualizationOperations(root=tmp_path / "operations", consumer_clock=lambda: 0)
    body = source(operations)
    operation = operations.receipts[body["binding"]["id"]]
    operation.pop("revision_binding")
    operation.update(status="completed", consumer_managed=True)
    operations._persist(operation)
    scope = {"consumer_id": "00000000-0000-0000-0000-000000000001",
             "intent_revision": 1, "setup_id": "setup", "stream_id": "overview"}
    declared = operations.consumers.declare(**scope)
    operations.consumers.attach({key: declared[key] for key in INTENT_FIELDS}, operation)
    review, runtime = service(operations, root=tmp_path / "reviews")
    first = await review.submit(**body)
    await finish(review)
    result = review.get(first["id"])
    assert result["correction_advisory"]["eligible_now"] is True
    parent = result["correction_advisory"]["expected_parent"]
    operations._reserve_correction(parent, fingerprint({"accepted": "earlier feedback"}),
        {"id": fingerprint({"accepted": "earlier correction"})})

    operations.consumers.declare(**{**scope, "intent_revision": 2, "setup_id": "company"})
    returned = operations.consumers.declare(**{**scope, "intent_revision": 3})
    operations.consumers.attach({key: returned[key] for key in INTENT_FIELDS}, operation)
    switched = review.get(first["id"])
    assert switched["current"] is True
    assert switched["correction_advisory"]["eligible_now"] is False
    assert switched["report"] == result["report"] and len(runtime.calls) == 1

    restored_operations = VisualizationOperations(root=tmp_path / "operations", consumer_clock=lambda: 0)
    restored_review, restored_runtime = service(restored_operations, root=tmp_path / "reviews")
    restored = restored_review.get(first["id"])
    assert restored["current"] is True
    assert restored["correction_advisory"]["eligible_now"] is False
    assert restored["report"] == result["report"] and not restored_runtime.calls
    assert restored_operations._correction_reservation(parent["operation_id"]) == operations._correction_reservation(parent["operation_id"])


@pytest.mark.asyncio
@pytest.mark.parametrize("reason", ["correction-child", "unreadable-reservation", "missing-authority"])
async def test_review_cannot_advertise_correction_when_budget_is_spent_or_unknown(reason):
    operations = Operations()
    body = source(operations)
    if reason == "correction-child":
        operations.receipts[body["binding"]["id"]]["correction_parent"] = {"operation_id": "a" * 64}
    elif reason == "unreadable-reservation":
        def unavailable(_identity):
            raise ValueError("The retained correction reservation is unavailable.")
        operations._correction_reservation = unavailable
    else:
        operations._correction_reservation = None
    review, _ = service(operations)
    first = await review.submit(**body)
    await finish(review)
    result = review.get(first["id"])
    assert result["current"] is True
    assert result["report"]["verdict"] == "needs_revision"
    assert result["correction_advisory"]["eligible_now"] is False
    assert not operations.mutations


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["scope", "code", "blueprint", "model", "generation", "reference", "renderer", "revision", "dimensions", "image-hash", "image-size", "selection", "crc", "runtime-size"])
async def test_invalid_evidence_rejected_before_any_helper_call(change):
    operations = Operations()
    body = source(operations)
    review, runtime = service(operations)
    if change == "scope": body["binding"]["setup_id"] = "other"
    elif change == "code": body["binding"]["code_hash"] = "0" * 64
    elif change == "blueprint": body["blueprint"]["project"]["name"] = "Changed source"
    elif change == "model": body["model"]["nodes"][0]["name"] = "Changed graph"
    elif change == "generation": body["binding"]["source_generation"] = 2
    elif change == "reference": body["binding"]["reference_hash"] = "0" * 64
    elif change == "renderer": body["renderer_hash"] = "0" * 64
    elif change == "revision": body["binding"]["revision_binding"] = {"version": "other"}
    elif change == "dimensions": body["frames"][0]["width"] += 1
    elif change == "image-hash": body["frames"][0]["png_hash"] = "0" * 64
    elif change == "image-size": body["frames"][0]["png_base64"] = "A" * (MAX_PNG_BYTES * 2)
    elif change == "selection": body["frames"][0]["selected_id"] = "invented:worker"
    elif change == "runtime-size": body["frames"][0]["runtime_activity"] = {"text": "x" * 16001}
    else:
        damaged = bytearray(base64.b64decode(body["frames"][0]["png_base64"]))
        damaged[30] ^= 1
        body["frames"][0]["png_base64"] = base64.b64encode(damaged).decode()
    with pytest.raises(ValueError):
        await review.submit(**body)
    assert not runtime.calls and not review.tasks and not review.receipts


def test_fractional_css_geometry_and_DPR_validate_actual_backing_pixels():
    operations = Operations()
    body = source(operations)
    body["frames"] = [frame(width=389.5, height=299.25, ratio=1.5)]
    snapshot, _ = checked_submission(operations, **body)
    assert snapshot["frames"][0]["width"] == 389.5


@pytest.mark.asyncio
async def test_scope_cannot_switch_owner_or_claim_another_helpers_review():
    operations = Operations()
    body = source(operations)
    review, runtime = service(operations)
    runtime.release = asyncio.Event()
    first = await review.submit(**body)
    second = await review.submit(**{**body, "frames": [frame("frame-two")]})
    callbacks, other_scope = ScopedReviewCallbacks(review), ScopedReviewCallbacks(review)
    claim = {"review_id": first["id"], "capability": review.receipts[first["id"]]["capability"]}
    await callbacks.inspect_visualization_review(claim)
    with pytest.raises(ValueError): await other_scope.inspect_visualization_review(claim)
    with pytest.raises(ValueError):
        await callbacks.inspect_visualization_review({"review_id": second["id"], "capability": review.receipts[second["id"]]["capability"]})
    with pytest.raises(ValueError): await callbacks.read_visualization_frame({"review_id": second["id"], "frame_id": "frame-two"})
    await review.shutdown()


@pytest.mark.asyncio
async def test_publication_requires_prior_image_result_receipt_not_handler_entry():
    operations = Operations()
    body = source(operations)
    review, runtime = service(operations)
    runtime.release = asyncio.Event()
    first = await review.submit(**body)
    callbacks = ScopedReviewCallbacks(review)
    await callbacks.inspect_visualization_review({"review_id": first["id"], "capability": review.receipts[first["id"]]["capability"]})
    with pytest.raises(ValueError): await callbacks.publish_visualization_review(report(first["id"], {}))
    image_result = await callbacks.read_visualization_frame({"review_id": first["id"], "frame_id": "frame-one"})
    with pytest.raises(ValueError): await callbacks.publish_visualization_review(report(first["id"], {"frame-one": "0" * 32}))
    token = json.loads(image_result.blocks[0]["text"])["frame_receipt"]
    assert (await callbacks.publish_visualization_review(report(first["id"], {"frame-one": token})))["valid"]
    changed = report(first["id"], {"frame-one": token}, verdict="acceptable")
    with pytest.raises(ValueError): await callbacks.publish_visualization_review(changed)
    await review.shutdown()


@pytest.mark.asyncio
async def test_missing_runtime_and_narration_only_result_are_inconclusive():
    operations = Operations()
    body = source(operations)
    disconnected = VisualizationReviewService(runtime=lambda: None, operations=operations, expected_renderer_hash=RENDERER)
    first = await disconnected.submit(**body)
    await finish(disconnected)
    assert disconnected.get(first["id"])["status"] == "inconclusive"
    review, runtime = service(operations)
    runtime.publish = False
    second = await review.submit(**body)
    await finish(review)
    assert review.get(second["id"])["status"] == "inconclusive" and "report" not in review.get(second["id"])


@pytest.mark.asyncio
async def test_shutdown_before_dispatch_does_not_leave_a_permanent_reviewing_receipt(tmp_path):
    operations = Operations()
    body = source(operations)
    review, runtime = service(operations, root=tmp_path)
    receipt = await review.submit(**body)
    await review.shutdown()
    assert not runtime.calls and not review.tasks
    assert review.get(receipt["id"])["status"] == "inconclusive"


@pytest.mark.asyncio
async def test_saved_evidence_and_published_report_tampering_are_refused(tmp_path):
    operations = Operations()
    body = source(operations)
    review, runtime = service(operations, root=tmp_path)
    receipt = await review.submit(**body)
    await finish(review)
    path = tmp_path / (receipt["id"] + ".json")
    saved = json.loads(path.read_text())
    changed = deepcopy(saved)
    changed["report"]["summary"] = "Invented replacement pass"
    path.write_text(json.dumps(changed))
    restored, _ = service(operations, root=tmp_path)
    with pytest.raises(ValueError, match="publication changed"):
        restored.get(receipt["id"])
    changed = deepcopy(saved)
    changed["snapshot"]["frames"][0]["time"] = 99
    path.write_text(json.dumps(changed))
    with pytest.raises(ValueError, match="evidence changed"):
        restored.get(receipt["id"])


@pytest.mark.asyncio
async def test_returned_metadata_cannot_mutate_frozen_runtime_observation():
    operations = Operations()
    body = source(operations)
    body["frames"][0]["runtime_activity"] = {"activeNodeIds": [body["model"]["nodes"][0]["id"]]}
    review, runtime = service(operations)
    receipt = await review.submit(**body)
    receipt["frames"][0]["runtime_activity"]["activeNodeIds"].append("invented")
    assert len(review.receipts[receipt["id"]]["snapshot"]["frames"][0]["runtime_activity"]["activeNodeIds"]) == 1
    await review.shutdown()


@pytest.mark.asyncio
async def test_async_runtime_provider_is_resolved_once_and_uses_native_helper_contract():
    operations, runtime, provider_calls = Operations(), Runtime(), []
    async def provider():
        provider_calls.append(True)
        await asyncio.sleep(0)
        return runtime
    review = VisualizationReviewService(runtime=provider, operations=operations, expected_renderer_hash=RENDERER)
    runtime.service = review
    receipt = await review.submit(**source(operations))
    await finish(review)
    assert len(provider_calls) == 1 and len(runtime.calls) == 1 and runtime.images
    assert review.get(receipt["id"])["status"] == "reviewed"


@pytest.mark.asyncio
async def test_same_helper_can_correct_invalid_publication_after_sixty_second_transport_default():
    from meerkat_meta.runtime import MetaRuntime

    operations, native = Operations(), Runtime()

    class CriticTransport:
        request_timeout = 60.0

        def __init__(self):
            self.calls = []
            self.wait = None
            self.elapsed = 0

        async def send_async(self, request, *, timeout=None):
            self.calls.append(request)
            self.wait = self.request_timeout if timeout is None else timeout
            params = request["params"]
            assert request["method"] == "meta/spawn_helper" and "timeout" not in params
            result = await native.spawn_helper(params["agent_identity"], params["task"], **params["options"], result_label=params["result_label"], max_text_bytes=params["max_text_bytes"])
            return {"result": {"output": result.output, "session_id": result.session_id, "tokens_used": result.tokens_used}}

    runtime = MetaRuntime.__new__(MetaRuntime)
    runtime._transport = transport = CriticTransport()
    review = VisualizationReviewService(runtime=runtime, operations=operations, expected_renderer_hash=RENDERER)
    native.service = review

    async def reject_then_continue(call, review_id, receipts):
        transport.elapsed = 47
        contradictory = report(review_id, receipts)
        contradictory["verdict"] = "acceptable"
        with pytest.raises(ValueError, match="contradicts"):
            await call("publish_visualization_review", contradictory)
        assert review.receipts[review_id]["report"] is None
        # Model the same helper's correction at 76 seconds. The fake transport
        # enforces its selected wait horizon without sleeping for a minute.
        transport.elapsed = 76
        if transport.wait < transport.elapsed:
            raise RuntimeError("The transport abandoned the existing helper.")
        assert review.receipts[review_id]["helper_state"] == "attempted"

    native.before_publication = reject_then_continue
    receipt = await review.submit(**source(operations))
    await finish(review)
    result = review.get(receipt["id"])
    assert 76 < transport.wait <= 180 and transport.request_timeout == 60
    assert len(transport.calls) == len(native.calls) == 1
    assert result["helper_state"] == "returned" and result["status"] == "reviewed"
    assert result["report"]["verdict"] == "needs_revision"
    assert not review.active_reviews
    assert (await review.submit(**source(operations)))["id"] == receipt["id"]
    assert len(transport.calls) == 1


@pytest.mark.asyncio
async def test_failed_async_bootstrap_is_terminal_without_dispatch_or_retry(tmp_path):
    operations, calls = Operations(), []
    async def provider():
        calls.append(True)
        raise OSError("Synthetic bootstrap failure.")
    body = source(operations)
    review = VisualizationReviewService(runtime=provider, operations=operations, expected_renderer_hash=RENDERER, root=tmp_path)
    first = await review.submit(**body)
    await finish(review)
    again = await review.submit(**body)
    assert first["id"] == again["id"] and len(calls) == 1
    assert again["status"] == "inconclusive" and again["helper_state"] == "not_dispatched"
    assert not review.active_reviews and not review.tasks


@pytest.mark.asyncio
async def test_native_timeout_retains_unknown_owner_and_exact_evidence_never_respawns(tmp_path):
    operations = Operations()
    review, runtime = service(operations, root=tmp_path)
    runtime.release = TransportTimedOut()
    body = source(operations)
    first = await review.submit(**body)
    await finish(review)
    result = review.get(first["id"])
    assert result["status"] == "inconclusive" and result["helper_state"] == "result_unknown"
    assert first["id"] in review.active_reviews
    saved = json.loads((tmp_path / (first["id"] + ".json")).read_text())
    assert saved["failure_diagnostic"] == {"category": "timeout", "stage": "post_dispatch",
        "exception_kind": "timeout", "boundary": "helper_await"}
    assert saved["dispatch_started"] is True
    assert review.publication_reviews[fingerprint(body["binding"])] == {first["id"]}
    assert "failure_diagnostic" not in result
    restored, other_runtime = service(operations, root=tmp_path)
    assert (await restored.submit(**body))["id"] == first["id"]
    assert restored.receipts[first["id"]]["failure_diagnostic"] == saved["failure_diagnostic"]
    assert first["id"] in json.loads((tmp_path / "active-reviews.json").read_text())["review_ids"]
    assert not other_runtime.calls and len(runtime.calls) == 1


@pytest.mark.asyncio
async def test_deadline_expiring_during_dispatch_binding_releases_slot_without_native_request(monkeypatch):
    operations = Operations()
    review, runtime = service(operations, timeout=0.01)
    persist = review._persist
    def slow_dispatch_binding(receipt):
        persist(receipt)
        if receipt.get("dispatch_started") and receipt.get("helper_state") == "attempted":
            # Synchronous durable writes can exhaust the deadline before the
            # task reaches its first native await.
            time.sleep(0.02)
    monkeypatch.setattr(review, "_persist", slow_dispatch_binding)
    receipt = await review.submit(**source(operations))
    await finish(review)
    result = review.get(receipt["id"])
    assert result["helper_state"] == "not_dispatched" and result["status"] == "inconclusive"
    assert review.receipts[receipt["id"]]["dispatch_started"] is False
    assert runtime.calls == [] and not review.active_reviews


@pytest.mark.asyncio
async def test_timeout_in_async_provider_does_not_claim_native_dispatch_or_hold_slot():
    operations, calls = Operations(), []
    async def provider():
        calls.append(True)
        await asyncio.Event().wait()
    review = VisualizationReviewService(runtime=provider, operations=operations, expected_renderer_hash=RENDERER, timeout=0.01)
    receipt = await review.submit(**source(operations))
    await finish(review)
    assert len(calls) == 1 and not review.active_reviews
    assert review.get(receipt["id"])["helper_state"] == "not_dispatched"


@pytest.mark.asyncio
async def test_active_review_admission_is_atomic_dedupes_first_and_persists_unknown_owners(tmp_path):
    operations = Operations()
    bodies = [source(operations, letter) for letter in ("A", "B", "C")]
    review, runtime = service(operations, root=tmp_path)
    runtime.release = asyncio.Event()
    submissions = await asyncio.gather(*(review.submit(**body) for body in bodies), return_exceptions=True)
    assert sum(isinstance(item, ReviewAdmissionUnavailable) for item in submissions) == 1
    assert len(review.receipts) == len(review.active_reviews) == 2
    await asyncio.sleep(0)
    assert len(runtime.calls) == 2
    first = await review.submit(**bodies[0])
    assert first["id"] == submissions[0]["id"] and len(runtime.calls) == 2
    denied_id = checked_submission(operations, **bodies[2])[1]
    assert not (tmp_path / (denied_id + ".json")).exists()
    await review.shutdown()
    restored, other_runtime = service(operations, root=tmp_path)
    assert (await restored.submit(**bodies[0]))["helper_state"] == "result_unknown"
    with pytest.raises(ReviewAdmissionUnavailable, match="active review slot"):
        await restored.submit(**bodies[2])
    assert not other_runtime.calls and len(restored.active_reviews) == 2


@pytest.mark.asyncio
async def test_publication_attempt_limit_survives_restart_and_completion_releases_active_slots(tmp_path):
    operations = Operations()
    body = source(operations)
    review, runtime = service(operations, root=tmp_path)
    first = await review.submit(**body)
    await finish(review)
    assert not review.active_reviews
    second_body = deepcopy(body)
    second_body["frames"][0]["time"] = 12
    second = await review.submit(**second_body)
    await finish(review)
    assert first["id"] != second["id"] and len(runtime.calls) == 2 and not review.active_reviews
    third_body = deepcopy(body)
    third_body["frames"][0]["time"] = 24
    restored, other_runtime = service(operations, root=tmp_path)
    assert (await restored.submit(**body))["id"] == first["id"]
    with pytest.raises(ReviewAdmissionUnavailable, match="attempt limit"):
        await restored.submit(**third_body)
    denied_id = checked_submission(operations, **third_body)[1]
    assert not (tmp_path / (denied_id + ".json")).exists() and not other_runtime.calls
    next_body = source(operations, "B")
    await restored.submit(**next_body)
    await finish(restored)
    assert len(other_runtime.calls) == 1 and not restored.active_reviews


@pytest.mark.asyncio
@pytest.mark.parametrize("malformation", ["frame", "source-path", "summary", "contradiction", "findings", "fields", "status", "exposure"])
async def test_retained_report_replays_bounded_contract_even_with_recomputed_report_hash(tmp_path, malformation):
    operations = Operations()
    review, _ = service(operations, root=tmp_path)
    receipt = await review.submit(**source(operations))
    await finish(review)
    path = tmp_path / (receipt["id"] + ".json")
    saved = json.loads(path.read_text())
    if malformation == "frame": saved["report"]["findings"][0]["frame_id"] = "unseen-frame"
    elif malformation == "source-path": saved["report"]["findings"][0]["source_paths"] = ["/model/nodes/999"]
    elif malformation == "summary": saved["report"]["summary"] = "x" * 1601
    elif malformation == "contradiction": saved["report"]["verdict"] = "acceptable"
    elif malformation == "findings": saved["report"]["findings"] *= 13
    elif malformation == "fields": saved["report"]["invented"] = True
    elif malformation == "status": saved["status"] = "reviewing"
    else: saved["report_binding"]["frame_receipt_hashes"] = {}
    saved["report_binding"]["report_hash"] = fingerprint(saved["report"])
    path.write_text(json.dumps(saved))
    restored, runtime = service(operations, root=tmp_path)
    with pytest.raises(ValueError):
        restored.get(receipt["id"])
    assert not runtime.calls and not restored.tasks


@pytest.mark.asyncio
async def test_advisory_feedback_is_canonical_UTF16_bounded_and_stable_after_restore(tmp_path):
    operations = Operations()
    review, runtime = service(operations, root=tmp_path)
    runtime.release = asyncio.Event()
    first = await review.submit(**source(operations))
    callbacks = ScopedReviewCallbacks(review)
    await callbacks.inspect_visualization_review({"review_id": first["id"], "capability": review.receipts[first["id"]]["capability"]})
    image = await callbacks.read_visualization_frame({"review_id": first["id"], "frame_id": "frame-one"})
    token = json.loads(image.blocks[0]["text"])["frame_receipt"]
    proposal = report(first["id"], {"frame-one": token})
    proposal["findings"][0]["observation"] = "Names\n\t overlap\u00a0\u2028and\u2009edges " + "\U0001f642" * 420
    proposal["findings"][0]["repair_hint"] = "\ufeff Keep\t\n names readable. "
    await callbacks.publish_visualization_review(proposal)
    advisory = review.get(first["id"])["correction_advisory"]["feedback"]
    assert "Names overlap and edges" in advisory and not re.search(r"[\n\t\u00a0\u2028\u2009\ufeff]", advisory)
    assert len(advisory.encode("utf-16-le")) <= 2000 and advisory.endswith("\U0001f642")
    assert advisory == _advisory_feedback(advisory)
    restored, other_runtime = service(operations, root=tmp_path)
    assert restored.get(first["id"])["correction_advisory"]["feedback"] == advisory
    assert not other_runtime.calls
    await review.shutdown()


def test_advisory_feedback_drops_controls_and_never_splits_an_emoji():
    assert _advisory_feedback(" \0one\x7f\x85two\ufeff ") == "one two"
    assert _advisory_feedback("x" * 999 + "\U0001f642") == "x" * 999


@pytest.mark.asyncio
async def test_admission_write_gap_retains_recoverable_not_dispatched_receipt(tmp_path, monkeypatch):
    operations = Operations()
    body = source(operations)
    review, runtime = service(operations, root=tmp_path)
    write = review._write_index
    def fail_reservation(name, identities):
        if name == "active-reviews.json" and identities:
            # Reproduce a crash after an active index reached disk. Its exact
            # receipt must already exist with no native dispatch begun.
            write(name, identities)
            raise OSError("Synthetic interruption after reservation write.")
        write(name, identities)
    monkeypatch.setattr(review, "_write_index", fail_reservation)
    with pytest.raises(OSError):
        await review.submit(**body)
    identity = checked_submission(operations, **body)[1]
    retained = json.loads((tmp_path / (identity + ".json")).read_text())
    assert retained["dispatch_started"] is False and not runtime.calls and not review.tasks
    restored, other_runtime = service(operations, root=tmp_path)
    assert (await restored.submit(**body))["helper_state"] == "not_dispatched"
    await restored.submit(**source(operations, "B"))
    await finish(restored)
    assert len(other_runtime.calls) == 1 and not restored.active_reviews


OWNER_A, OWNER_B, OWNER_OTHER = "a" * 32, "b" * 32, "c" * 32


@pytest.mark.asyncio
async def test_confirmed_exact_owner_recovers_two_unknown_slots_across_restart_without_respawn(tmp_path):
    operations = Operations()
    review, runtime = service(operations, root=tmp_path, runtime_owner_id=lambda exact: OWNER_A if exact is runtime else None)
    runtime.release = TransportTimedOut()
    bodies = [source(operations, letter) for letter in ("A", "B")]
    admitted = [await review.submit(**body) for body in bodies]
    await finish(review)
    assert len(runtime.calls) == len(review.active_reviews) == 2
    before = [review.get(item["id"]) for item in admitted]
    restored, other_runtime = service(operations, root=tmp_path, runtime_owner_id=lambda exact: OWNER_B if exact is other_runtime else None)
    confirmed = await restored.confirm_runtime_terminated(OWNER_A)
    assert set(confirmed["released_review_ids"]) == {item["id"] for item in admitted}
    assert not restored.active_reviews and not other_runtime.calls
    for body, retained in zip(bodies, before):
        assert await restored.submit(**body) == retained
        saved = json.loads((tmp_path / (retained["id"] + ".json")).read_text())
        assert saved["helper_state"] == "result_unknown" and saved["status"] == "inconclusive"
        assert saved["runtime_termination"]["owner_id"] == saved["dispatch_owner_id"] == OWNER_A
    await restored.submit(**source(operations, "C"))
    await finish(restored)
    assert len(other_runtime.calls) == 1


@pytest.mark.asyncio
async def test_wrong_or_stale_owner_confirmation_cannot_release_other_live_generation(tmp_path):
    operations, runtime_A, runtime_B = Operations(), Runtime(), Runtime()
    current = {"runtime": runtime_A}
    owners = {runtime_A: OWNER_A, runtime_B: OWNER_B}
    # No wall-clock budget decides whether a review is dispatched: each native
    # request is sent and then times out in its transport, so both stay live.
    review = VisualizationReviewService(runtime=lambda: current["runtime"], runtime_owner_id=lambda exact: owners.get(exact),
        operations=operations, expected_renderer_hash=RENDERER, root=tmp_path)
    runtime_A.service = runtime_B.service = review
    runtime_A.release = runtime_B.release = TransportTimedOut()
    first = await review.submit(**source(operations, "A"))
    await finish(review)
    current["runtime"] = runtime_B
    second = await review.submit(**source(operations, "B"))
    await finish(review)
    assert review.active_reviews == {first["id"], second["id"]}
    assert not (await review.confirm_runtime_terminated(OWNER_OTHER))["released_review_ids"]
    assert review.active_reviews == {first["id"], second["id"]}
    assert (await review.confirm_runtime_terminated(OWNER_A))["released_review_ids"] == [first["id"]]
    assert review.active_reviews == {second["id"]}
    assert not (await review.confirm_runtime_terminated(OWNER_A))["released_review_ids"]
    assert review.active_reviews == {second["id"]}
    assert (await review.confirm_runtime_terminated(OWNER_B))["released_review_ids"] == [second["id"]]
    assert not review.active_reviews


@pytest.mark.asyncio
async def test_confirmed_owner_tombstone_blocks_future_dispatch_to_that_boot(tmp_path):
    operations = Operations()
    review, runtime = service(operations, root=tmp_path, runtime_owner_id=lambda exact: OWNER_A)
    await review.confirm_runtime_terminated(OWNER_A)
    receipt = await review.submit(**source(operations))
    await finish(review)
    assert not runtime.calls and not review.active_reviews
    assert review.get(receipt["id"])["helper_state"] == "not_dispatched"
    restored, other_runtime = service(operations, root=tmp_path, runtime_owner_id=lambda exact: OWNER_A)
    newer = await restored.submit(**source(operations, "B"))
    await finish(restored)
    assert not other_runtime.calls and restored.get(newer["id"])["helper_state"] == "not_dispatched"


@pytest.mark.asyncio
@pytest.mark.parametrize("owner_result", [None, "invalid", "A" * 32, "0" * 31])
async def test_configured_owner_callback_requires_valid_identity_before_native_dispatch(owner_result):
    operations = Operations()
    review, runtime = service(operations, runtime_owner_id=lambda exact: owner_result)
    receipt = await review.submit(**source(operations))
    await finish(review)
    assert not runtime.calls and not review.active_reviews
    assert review.get(receipt["id"])["helper_state"] == "not_dispatched"


@pytest.mark.asyncio
async def test_owner_callback_failure_is_terminal_and_exact_runtime_object_is_supplied():
    operations, seen = Operations(), []
    def owner(exact):
        seen.append(exact)
        raise ValueError("Host cannot establish this exact runtime owner.")
    review, runtime = service(operations, runtime_owner_id=owner)
    body = source(operations)
    receipt = await review.submit(**body)
    await finish(review)
    assert seen == [runtime] and not runtime.calls and not review.active_reviews
    assert (await review.submit(**body))["id"] == receipt["id"] and seen == [runtime]


@pytest.mark.asyncio
async def test_unbound_legacy_unknown_receipt_stays_held_after_unrelated_owner_confirmation(tmp_path):
    operations = Operations()
    review, runtime = service(operations, root=tmp_path, max_active_reviews=1)
    runtime.release = TransportTimedOut()
    receipt = await review.submit(**source(operations))
    await finish(review)
    path = tmp_path / (receipt["id"] + ".json")
    legacy = json.loads(path.read_text())
    legacy.pop("dispatch_owner_id")
    path.write_text(json.dumps(legacy))
    restored, other_runtime = service(operations, root=tmp_path, max_active_reviews=1, runtime_owner_id=lambda exact: OWNER_A)
    assert not (await restored.confirm_runtime_terminated(OWNER_A))["released_review_ids"]
    assert restored.active_reviews == {receipt["id"]}
    with pytest.raises(ReviewAdmissionUnavailable):
        await restored.submit(**source(operations, "B"))
    assert not other_runtime.calls


@pytest.mark.asyncio
async def test_owner_binding_is_durable_before_RPC_and_failed_binding_write_never_dispatches(tmp_path, monkeypatch):
    operations = Operations()
    review, runtime = service(operations, root=tmp_path, runtime_owner_id=lambda exact: OWNER_A)
    write, observed = review._persist, []
    def interrupted_binding(receipt):
        write(receipt)
        if receipt["dispatch_started"] and not observed:
            saved = json.loads((tmp_path / (receipt["id"] + ".json")).read_text())
            observed.append((saved["dispatch_owner_id"], bool(runtime.calls)))
            raise OSError("Synthetic write-gap interruption before native RPC.")
    monkeypatch.setattr(review, "_persist", interrupted_binding)
    first = await review.submit(**source(operations))
    await finish(review)
    assert observed == [(OWNER_A, False)] and not runtime.calls and not review.active_reviews
    assert review.get(first["id"])["helper_state"] == "not_dispatched"
    assert not (await review.confirm_runtime_terminated(OWNER_A))["released_review_ids"]


@pytest.mark.asyncio
async def test_termination_tombstone_recovers_index_write_gap_on_restart(tmp_path, monkeypatch):
    operations = Operations()
    review, runtime = service(operations, root=tmp_path, runtime_owner_id=lambda exact: OWNER_A)
    runtime.release = TransportTimedOut()
    first = await review.submit(**source(operations))
    await finish(review)
    write = review._write_index
    def interrupted_release(name, identities):
        if name == "active-reviews.json" and not identities:
            raise OSError("Synthetic interruption before active index release write.")
        write(name, identities)
    monkeypatch.setattr(review, "_write_index", interrupted_release)
    with pytest.raises(OSError):
        await review.confirm_runtime_terminated(OWNER_A)
    assert (tmp_path / ("runtime-terminated-" + OWNER_A + ".json")).exists()
    restored, other_runtime = service(operations, root=tmp_path, runtime_owner_id=lambda exact: OWNER_B)
    await restored.submit(**source(operations, "B"))
    await finish(restored)
    assert not restored.active_reviews and len(other_runtime.calls) == 1
    assert restored.get(first["id"])["helper_state"] == "result_unknown"


@pytest.mark.asyncio
async def test_owner_termination_preserves_checked_report_and_unknown_result_history(tmp_path):
    operations = Operations()
    review, runtime = service(operations, root=tmp_path, runtime_owner_id=lambda exact: OWNER_A)
    runtime.raise_after_publication = True
    body = source(operations)
    first = await review.submit(**body)
    await finish(review)
    before = review.get(first["id"])
    assert before["status"] == "reviewed" and before["helper_state"] == "result_unknown"
    await review.confirm_runtime_terminated(OWNER_A)
    assert review.get(first["id"]) == before and not review.active_reviews
    restored, other_runtime = service(operations, root=tmp_path)
    assert await restored.submit(**body) == before and not other_runtime.calls


@pytest.mark.asyncio
async def test_positive_owner_end_cancels_only_its_pending_observation_and_preserves_terminal_receipt(tmp_path):
    operations = Operations()
    review, runtime = service(operations, root=tmp_path, runtime_owner_id=lambda exact: OWNER_A)
    runtime.release = asyncio.Event()
    body = source(operations)
    first = await review.submit(**body)
    await runtime.started.wait()
    assert review.get(first["id"])["status"] == "reviewing"
    await review.confirm_runtime_terminated(OWNER_A)
    await asyncio.gather(*list(review.tasks.values()), return_exceptions=True)
    assert not review.active_reviews and not review.tasks
    assert review.get(first["id"])["status"] == "inconclusive"
    assert review.get(first["id"])["helper_state"] == "result_unknown"
    assert (await review.submit(**body))["id"] == first["id"] and len(runtime.calls) == 1


@pytest.mark.asyncio
async def test_earlier_renderer_review_keeps_slot_custody_but_never_reuses_its_evidence(tmp_path):
    operations = Operations()
    review, runtime = service(operations, root=tmp_path, runtime_owner_id=lambda exact: OWNER_A)
    runtime.release = TransportTimedOut()
    earlier = await review.submit(**source(operations))
    await finish(review)
    assert review.active_reviews == {earlier["id"]}
    # A release that changes the trusted renderer bytes changes their hash.
    renderer, upgraded_runtime = fingerprint({"renderer": "next release"}), Runtime()
    upgraded = VisualizationReviewService(runtime=upgraded_runtime, operations=operations, root=tmp_path,
        expected_renderer_hash=renderer, runtime_owner_id=lambda exact: OWNER_B)
    upgraded_runtime.service = upgraded
    current = await upgraded.submit(**{**source(operations, "B"), "renderer_hash": renderer})
    await finish(upgraded)
    assert upgraded.get(current["id"])["status"] == "reviewed" and upgraded.active_reviews == {earlier["id"]}
    assert (await upgraded.confirm_runtime_terminated(OWNER_A))["released_review_ids"] == [earlier["id"]]
    assert not upgraded.active_reviews and not json.loads((tmp_path / "active-reviews.json").read_text())["review_ids"]
    with pytest.raises(ValueError, match="another trusted bridge version"):
        upgraded.get(earlier["id"])


@pytest.mark.asyncio
async def test_real_inspect_output_teaches_valid_source_pointers_and_empty_visual_claims():
    operations = Operations()
    review, runtime = service(operations)
    runtime.release = asyncio.Event()
    receipt = await review.submit(**source(operations))
    callbacks = ScopedReviewCallbacks(review)
    inspected = await callbacks.inspect_visualization_review({"review_id": receipt["id"], "capability": review.receipts[receipt["id"]]["capability"]})
    guidance = inspected["source_path_guidance"]
    assert guidance["format"] == "RFC 6901 JSON pointers rooted at /blueprint or /model"
    assert guidance["empty_allowed"] is True
    assert 2 <= len(guidance["valid_examples"]) <= 5
    assert "/blueprint/project/name" in guidance["valid_examples"] and "/model/nodes/0/name" in guidance["valid_examples"]
    candidate = {key: value for key, value in report(receipt["id"], {}).items() if key in {"verdict", "summary", "findings"}}
    for path in guidance["valid_examples"]:
        candidate["findings"][0]["source_paths"] = [path]
        assert module.checked_report(inspected, candidate)["findings"][0]["source_paths"] == [path]
    candidate["findings"][0]["source_paths"] = []
    assert module.checked_report(inspected, candidate)["findings"][0]["source_paths"] == []
    candidate["findings"][0]["source_paths"] = ["model.nodes[0].name"]
    with pytest.raises(ValueError, match="current source paths"):
        module.checked_report(inspected, candidate)
    await review.shutdown()
