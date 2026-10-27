"""Accepted animation source is available without polling its native tail first."""
import asyncio
import copy
import json
from types import SimpleNamespace

import pytest

from test_visualizer import CODE, inputs, service


@pytest.mark.asyncio
@pytest.mark.parametrize("restored_owner", [False, True])
async def test_bound_publication_skips_reads_and_retains_exact_pending_custody(
        service, monkeypatch, restored_owner):
    blueprint, model = inputs()
    binding = service.request_binding(blueprint, model)
    service._load()
    job = {**{key: value for key, value in binding.items() if key != "native_request_key"},
           "key": binding["native_request_key"], "blueprint": blueprint, "model": model,
           "idempotency_key": "already-admitted-work",
           "admission": {"cursor": "console:10", "interaction_id": "owned-native",
                         "session_id": "owned-session"},
           "owner_id": "earlier-host" if restored_owner else service.owner_id,
           "author_context_read": False, "examples_read": False, "publication": None}
    service.pending = job
    await service.inspect({"view": "overview", "request_key": job["key"], "include_author_references": True})
    accepted = await service.publish({"request_key": job["key"], "code": CODE,
                                      "summary": "An acknowledged scene."})
    assert accepted["valid"] is True
    original_admission = copy.deepcopy(job["admission"])

    async def no_native_read(*args, **kwargs):
        raise AssertionError("An already bound publication needs no native read.")

    monkeypatch.setattr("launchkit.architect._native_request", no_native_read)
    monkeypatch.setattr("httpx.AsyncClient.post", no_native_read)
    artifact = await service._finish()
    assert artifact["code"] == CODE
    assert artifact["native_run"] == {"status": "unknown", "interaction_id": "owned-native"}
    assert service.pending is job and not job.get("completed")
    saved = json.loads((service.root / "pending.json").read_text())
    assert saved["key"] == job["key"] and saved["admission"] == original_admission
    assert saved["idempotency_key"] == "already-admitted-work"

    reading, release = asyncio.Event(), asyncio.Event()
    calls = []

    async def native_tail(client, method, url, **kwargs):
        calls.append((method, url, kwargs["params"]["after"]))
        assert method == "get" and url.endswith("/console/timeline")
        reading.set()
        await release.wait()
        return SimpleNamespace(json=lambda: {"frames": [
            {"kind": "interaction_complete", "interaction_id": "owned-native",
             "cursor": "console:11"}]})

    monkeypatch.setattr("launchkit.architect._native_request", native_tail)
    observation = asyncio.create_task(service.observe_generation(job["key"]))
    await asyncio.wait_for(reading.wait(), 30)
    assert service.pending is job and job["admission"] == original_admission
    assert service.lock.locked() and not job.get("completed")
    release.set()
    observed = await asyncio.wait_for(observation, 1)
    assert observed["native_run"]["status"] == "completed"
    assert observed["native_admission"] == original_admission
    assert service.pending is None and not (service.root / "pending.json").exists()
    assert calls == [("get", service.architect.runtime.rust_http_base_url + "/console/timeline",
                      "console:10")]
