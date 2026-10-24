"""Host retention/lifecycle controls only; no native authority is simulated."""
import asyncio
from copy import deepcopy
from dataclasses import dataclass
import json
import sys
import threading
from types import ModuleType, SimpleNamespace

import pytest

from launchkit.connect_installer_caller import CustodyError
from launchkit.connect_application_startup import bind_native_operations
from test_application_extension import load_host


@pytest.fixture
def sdk(monkeypatch):
    @dataclass(frozen=True)
    class Ref:
        route: str
        original: str
        event_sha256: str

    @dataclass(frozen=True)
    class Evidence:
        domain: str
        evidence: str

    @dataclass(frozen=True)
    class Submission:
        original: Ref
        content: str
        transport_authentication: Evidence

    # Only the adapter's typed value boundary. The actual SDK/native target
    # validation and grants have their separate owner tests in Connect.
    module = ModuleType("meerkat_toolkit.host_control")
    module.PrivateOriginalRef = Ref
    module.PrivateOriginalSubmit = Submission
    module.PrivateSourceEvidence = Evidence
    # The real finite schema belongs to Connect. This fake exposes only its
    # pure boundary, allowing tests to prove refusal precedes app retention.
    module.validate_native_original_operations_request = deepcopy
    monkeypatch.setitem(sys.modules, "meerkat_toolkit.host_control", module)
    return SimpleNamespace(Ref=Ref, Evidence=Evidence, Submission=Submission, module=module)


def fixture(sdk, *, dynamic=False):
    calls, retained = [], []
    result = sdk.Ref("actual-added-route", "original-1", "a" * 64)
    submission = sdk.Submission(result, "Original text\nunchanged.", sdk.Evidence("transport", "proof"))
    request = {"not_before_ms": 1000, "expires_at_ms": 61000, "targets": [
        {"issuance_id": "peer-root", "target": {"kind": "peer", "route": "actual-other-route",
            "actions": ["message", "request"]}},
        {"issuance_id": "directory-root", "target": {"kind": "peer_directory"}},
        {"issuance_id": "recorder-root", "target": {"kind": "recorder_identity",
            "actions": ["read", "write", "delete"]}}]}

    def prepare(original, selected, **options):
        calls.append((original, deepcopy(selected), options))
        return original.original

    runtime = SimpleNamespace(host_control=SimpleNamespace(prepare_original_native_operations=prepare))
    context = SimpleNamespace(runtime=runtime, stop_event=asyncio.Event())
    callbacks = SimpleNamespace(_current=lambda: None)
    app = SimpleNamespace(context=context, dynamic=dynamic, _application_callbacks=callbacks)
    owner = SimpleNamespace(runtime=runtime, application=app, callbacks=callbacks,
        _shutdown_task=None, _require_current=lambda: None)
    bind_native_operations(owner, app, runtime)

    async def retain(value):
        retained.append(value)

    return SimpleNamespace(owner=owner, app=app, runtime=runtime, context=context,
        adapter=context._native_original_operations, submission=submission, request=request,
        result=result, calls=calls, retained=retained, retain=retain)


@pytest.mark.asyncio
@pytest.mark.parametrize("dynamic", [False, True])
async def test_retains_exact_intent_then_prepares_without_submission_or_seed_lookup(sdk, dynamic):
    f = fixture(sdk, dynamic=dynamic)
    original_request = deepcopy(f.request)

    async def retain(value):
        assert not f.calls
        await f.retain(value)
        f.request["targets"][0]["target"]["route"] = "changed-by-caller"

    result = await f.adapter.prepare(f.submission, f.request, retain_request=retain, timeout=17.5)
    assert result == f.result
    saved = json.loads(f.retained[0])
    assert saved["request"] == original_request
    assert saved["submission"]["content"] == f.submission.content
    assert saved["submission"]["original"]["route"] == "actual-added-route"
    assert f.calls == [(f.submission, original_request, {"timeout": 17.5})]
    # Runtime has no submit API, roster, native key or authority constructor.
    assert not f.adapter._pending


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["owner-runtime", "context-runtime", "application", "callbacks",
    "shutdown", "stop", "private-client", "source", "retention-error"])
async def test_refuses_changed_owner_or_failed_retention_before_native_call(sdk, change):
    f = fixture(sdk)

    async def retain(value):
        await f.retain(value)
        if change == "owner-runtime": f.owner.runtime = object()
        elif change == "context-runtime": f.context.runtime = object()
        elif change == "application": f.owner.application = object()
        elif change == "callbacks": f.app._application_callbacks = object()
        elif change == "shutdown": f.owner._shutdown_task = object()
        elif change == "stop": f.context.stop_event.set()
        elif change == "private-client": f.runtime.host_control = object()
        elif change == "source":
            def stale(): raise CustodyError("source changed")
            f.owner._require_current = stale
        else: raise OSError("retention failed")

    with pytest.raises(OSError if change == "retention-error" else CustodyError):
        await f.adapter.prepare(f.submission, f.request, retain_request=retain)
    assert len(f.retained) == 1 and not f.calls


@pytest.mark.asyncio
@pytest.mark.parametrize("timeout", [0, -1, True, float("nan"), float("inf"), "60"])
async def test_invalid_timeout_never_enters_retention(sdk, timeout):
    f = fixture(sdk)
    with pytest.raises(ValueError):
        await f.adapter.prepare(f.submission, f.request, retain_request=f.retain, timeout=timeout)
    assert not f.calls and not f.retained


@pytest.mark.asyncio
async def test_older_sdk_refuses_without_retaining_or_submitting(sdk):
    f = fixture(sdk)
    f.runtime.host_control = object()
    with pytest.raises(CustodyError, match="does not support"):
        await f.adapter.prepare(f.submission, f.request, retain_request=f.retain)
    assert not f.retained


@pytest.mark.asyncio
async def test_native_refusal_is_not_converted_to_permission_or_retried(sdk):
    f = fixture(sdk)
    error = RuntimeError("native declined current route")

    def refuse(*args, **kwargs):
        f.calls.append("attempted")
        raise error

    f.runtime.host_control.prepare_original_native_operations = refuse
    with pytest.raises(RuntimeError) as refused:
        await f.adapter.prepare(f.submission, f.request, retain_request=f.retain)
    assert refused.value is error
    assert f.calls == ["attempted"] and len(f.retained) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("ending", ["cancel", "close"])
async def test_retention_is_joined_and_preparation_refused_after_cancellation_or_close(sdk, ending):
    f = fixture(sdk)
    entered, release = asyncio.Event(), asyncio.Event()

    async def retain(value):
        entered.set()
        await release.wait()
        await f.retain(value)

    task = asyncio.create_task(f.adapter.prepare(f.submission, f.request, retain_request=retain))
    await entered.wait()
    closing = None
    try:
        if ending == "cancel": task.cancel()
        else: closing = asyncio.create_task(f.adapter.close())
        await asyncio.sleep(0)
        assert not task.done()
        if closing: assert not closing.done()
    finally:
        release.set()
    with pytest.raises(asyncio.CancelledError if ending == "cancel" else CustodyError):
        await task
    if closing: await closing
    assert len(f.retained) == 1 and not f.calls


@pytest.mark.asyncio
async def test_cancel_and_stop_join_entered_sdk_worker_without_retry(sdk):
    f = fixture(sdk)
    loop = asyncio.get_running_loop()
    entered, release = asyncio.Event(), threading.Event()

    def blocked(*args, **kwargs):
        f.calls.append("entered")
        loop.call_soon_threadsafe(entered.set)
        assert release.wait(5), "test did not release SDK worker"
        f.calls.append("returned")
        return f.result

    f.runtime.host_control.prepare_original_native_operations = blocked
    task = asyncio.create_task(f.adapter.prepare(f.submission, f.request, retain_request=f.retain))
    await entered.wait()
    closing = None
    try:
        task.cancel()
        closing = asyncio.create_task(f.adapter.close())
        await asyncio.sleep(0)
        task.cancel()
        await asyncio.sleep(0)
        assert not task.done() and not closing.done()
    finally:
        release.set()
    with pytest.raises(asyncio.CancelledError): await task
    await closing
    assert f.calls == ["entered", "returned"]
    with pytest.raises(CustodyError):
        await f.adapter.prepare(f.submission, f.request, retain_request=f.retain)


@pytest.mark.asyncio
async def test_context_unavailable_before_selected_startup(tmp_path, monkeypatch):
    host = load_host(monkeypatch, tmp_path)
    context = host.ApplicationContext(tmp_path, {}, asyncio.Event())
    with pytest.raises(RuntimeError, match="connected selected"):
        await context.prepare_original_native_operations(object(), {}, retain_request=lambda value: None)


@pytest.mark.asyncio
async def test_application_stop_joins_before_closing_store(tmp_path, monkeypatch):
    host = load_host(monkeypatch, tmp_path)
    context = host.ApplicationContext(tmp_path, {}, asyncio.Event())
    app = host.Application(context)
    events = []

    async def close(): events.append("native-calls-joined")
    async def stop(): events.append("store-closed")

    context._native_original_operations = SimpleNamespace(close=close)
    app.integration = SimpleNamespace(stop=stop)
    await app.stop()
    assert events == ["native-calls-joined", "store-closed"]


def test_export_includes_native_operations_adapter():
    from launchkit.compiler import connect_application_finalizer_export
    files = {item.path: item.content for item in connect_application_finalizer_export()}
    assert "class NativeOriginalOperations" in files["launchkit/connect_application_startup.py"]
    assert "launchkit/connect_native_operations.py" not in files
    from launchkit.deployment_sources import _NATIVE_PATHS
    assert set(files) <= _NATIVE_PATHS
    from launchkit.deployment_updates import _RUNTIME_FRAMEWORK_V6_PATHS
    assert set(files) <= _RUNTIME_FRAMEWORK_V6_PATHS


@pytest.mark.asyncio
async def test_equal_request_key_order_retains_identical_bytes_for_its_same_original(sdk):
    f = fixture(sdk)
    def reorder(value):
        if isinstance(value, dict):
            return {key: reorder(value[key]) for key in reversed(value)}
        if isinstance(value, list):
            return [reorder(item) for item in value]
        return value
    await f.adapter.prepare(f.submission, f.request, retain_request=f.retain)
    await f.adapter.prepare(f.submission, reorder(f.request), retain_request=f.retain)
    assert f.retained[0] == f.retained[1]
    assert f.calls[0] == f.calls[1]


@pytest.mark.asyncio
async def test_sdk_shape_refusal_precedes_retention_and_native_preparation(sdk):
    f = fixture(sdk)
    f.request["targets"][0]["target"]["kind"] = "unsupported_scope"
    refusal = ValueError("selected SDK rejected the target")
    observed = []
    def validate(request):
        observed.append(deepcopy(request))
        raise refusal
    sdk.module.validate_native_original_operations_request = validate
    with pytest.raises(ValueError) as caught:
        await f.adapter.prepare(f.submission, f.request, retain_request=f.retain)
    assert caught.value is refusal
    assert observed == [f.request]
    assert not f.retained and not f.calls and not f.adapter._pending


@pytest.mark.asyncio
async def test_sdk_without_public_validation_refuses_before_retaining(sdk):
    f = fixture(sdk)
    del sdk.module.validate_native_original_operations_request
    with pytest.raises(CustodyError, match="request validation"):
        await f.adapter.prepare(f.submission, f.request, retain_request=f.retain)
    assert not f.retained and not f.calls


@pytest.mark.asyncio
@pytest.mark.parametrize("result", [None, [], "request", False])
async def test_sdk_nonobject_result_refuses_before_retention_or_native_preparation(sdk, result):
    f = fixture(sdk)
    sdk.module.validate_native_original_operations_request = lambda request: result
    with pytest.raises(CustodyError, match="validated request object"):
        await f.adapter.prepare(f.submission, f.request, retain_request=f.retain)
    assert not f.retained and not f.calls and not f.adapter._pending
