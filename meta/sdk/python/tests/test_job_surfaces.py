from __future__ import annotations

import pytest

from meerkat_meta import Meta, MetaRuntime


@pytest.mark.asyncio
async def test_jobs_and_monitors_use_canonical_domain_methods() -> None:
    runtime = MetaRuntime(Meta.builder()._config)
    calls: list[tuple[str, dict]] = []

    async def rpc(method: str, params: dict | None = None):
        calls.append((method, params or {}))
        return {"method": method}

    runtime._rpc = rpc  # type: ignore[method-assign]

    await runtime.jobs.get("job-1")
    await runtime.jobs.list(session_id="session-1", limit=25)
    await runtime.jobs.cancel("job-1")
    await runtime.jobs.progress("job-1")
    await runtime.jobs.result("job-1")
    await runtime.jobs.artifacts("job-1")
    await runtime.jobs.retry("job-1", retry_due_at_ms=123)
    await runtime.jobs.health()
    await runtime.jobs.subscribe(
        "job-1",
        subscription_id="sub-1",
        session_id="session-1",
        delivery={"kind": "event", "handling_mode": "steer"},
    )
    await runtime.jobs.unsubscribe("job-1", subscription_id="sub-1")
    await runtime.monitors.start(
        session_id="session-1",
        submission_key="monitor:lan",
        command="./scan --watch",
        timeout_secs=600,
        restart_class="non_resumable",
        delivery={"kind": "notification"},
        protocol="framed_jsonl",
        working_dir="/srv/example",
        max_line_bytes=4096,
    )

    assert calls == [
        ("jobs/get", {"job_id": "job-1"}),
        ("jobs/list", {"session_id": "session-1", "limit": 25}),
        ("jobs/cancel", {"job_id": "job-1"}),
        ("jobs/progress", {"job_id": "job-1"}),
        ("jobs/result", {"job_id": "job-1"}),
        ("jobs/artifacts", {"job_id": "job-1"}),
        ("jobs/retry", {"job_id": "job-1", "retry_due_at_ms": 123}),
        ("jobs/health", {}),
        (
            "jobs/subscribe",
            {
                "job_id": "job-1",
                "subscription_id": "sub-1",
                "session_id": "session-1",
                "delivery": {"kind": "event", "handling_mode": "steer"},
            },
        ),
        (
            "jobs/unsubscribe",
            {"job_id": "job-1", "subscription_id": "sub-1"},
        ),
        (
            "monitors/start",
            {
                "session_id": "session-1",
                "submission_key": "monitor:lan",
                "command": "./scan --watch",
                "timeout_secs": 600,
                "protocol": "framed_jsonl",
                "restart_class": "non_resumable",
                "delivery": {"kind": "notification"},
                "working_dir": "/srv/example",
                "max_line_bytes": 4096,
            },
        ),
    ]
