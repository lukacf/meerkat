"""Detached callback jobs: private wire metadata and host control-plane behavior."""
from __future__ import annotations

import asyncio
from typing import Any

import pytest

from meerkat_meta.agent_builder import CallbackDispatcher
from meerkat_meta.jobs import (
    DetachedJobContext,
    DetachedJobExecution,
    DetachedJobResult,
)
from meerkat_meta.models import SessionBuildOptions


AUTHORITY_1 = {"job_id": "job-1", "attempt_id": "attempt-1", "fence": 7}
AUTHORITY_2 = {"job_id": "job-1", "attempt_id": "attempt-2", "fence": 8}


def start_params(
    authority: dict[str, Any] = AUTHORITY_1,
    *,
    runner_handle: str = "callback:job-1:attempt:1",
) -> dict[str, Any]:
    return {
        "authority": dict(authority),
        "runner": {"name": "example.security_scan", "version": "1"},
        "restart_class": "non_resumable",
        "runner_handle": runner_handle,
        "runner_specification_ref": "blob-args",
        "arguments": {"target": "lan"},
        "credential_scopes": ["network.read"],
    }


class Builder:
    def __init__(self, runner: Any, *, profile_name: str = "network") -> None:
        self.runner = runner
        self.profile_name = profile_name

    async def build_agent(self, options: SessionBuildOptions) -> None:
        options.profile_name = self.profile_name
        options.register_tool(
            "security_scan",
            lambda _args: None,
            execution=DetachedJobExecution(
                runner="example.security_scan",
                version="1",
                restart_class="non_resumable",
                idempotency_scope="interaction_and_arguments",
                submission_timeout_ms=30_000,
                credential_scopes=("network.read",),
                handler=self.runner,
            ),
        )


async def register(
    dispatcher: CallbackDispatcher,
    runner: Any,
    *,
    profile_name: str = "network",
    scope_id: str = "build-1",
) -> dict[str, Any]:
    dispatcher.register_builder(Builder(runner, profile_name=profile_name))
    return await dispatcher.handle_callback(
        "callback/build_agent",
        {"options": {"scope_id": scope_id}},
    )


def test_detached_registration_emits_exact_private_execution_contract() -> None:
    async def runner(_context: DetachedJobContext) -> None:
        return None

    options = SessionBuildOptions(profile_name="network")
    options.register_tool(
        "security_scan",
        lambda _args: None,
        description="Scan the LAN",
        input_schema={"type": "object"},
        execution=DetachedJobExecution(
            runner="example.security_scan",
            version="1",
            restart_class="non_resumable",
            idempotency_scope="interaction_and_arguments",
            submission_timeout_ms=30_000,
            credential_scopes=("network.read",),
            handler=runner,
        ),
    )

    assert options.to_dict()["tools"] == [
        {
            "name": "security_scan",
            "description": "Scan the LAN",
            "input_schema": {"type": "object"},
            "execution": {
                "mode": "detached",
                "runner": {"name": "example.security_scan", "version": "1"},
                "restart_class": "non_resumable",
                "idempotency_scope": "interaction_and_arguments",
                "submission_timeout_ms": 30_000,
                "credential_scopes": ["network.read"],
            },
        }
    ]


@pytest.mark.asyncio
async def test_start_returns_before_work_and_reports_with_exact_authority() -> None:
    entered = asyncio.Event()
    release = asyncio.Event()
    rpc_calls: list[tuple[str, dict[str, Any]]] = []
    resolver_calls: list[tuple[str | None, tuple[str, ...]]] = []

    async def resolve_credentials(
        profile_name: str | None,
        scopes: tuple[str, ...],
    ) -> dict[str, str]:
        resolver_calls.append((profile_name, scopes))
        return {"token": "secret-current-attempt"}

    async def rpc(method: str, params: dict[str, Any]) -> dict[str, Any]:
        rpc_calls.append((method, params))
        return {"job": {"job_id": "job-1"}}

    async def runner(context: DetachedJobContext) -> DetachedJobResult:
        assert context.arguments == {"target": "lan"}
        assert context.credentials == {"token": "secret-current-attempt"}
        await context.progress(1, "started", observed_at_ms=101)
        entered.set()
        await release.wait()
        return DetachedJobResult(result_ref="artifact:scan")

    dispatcher = CallbackDispatcher()
    dispatcher.register_job_rpc(rpc)
    dispatcher.register_job_credential_resolver(resolve_credentials)
    await register(dispatcher, runner)

    result = await asyncio.wait_for(
        dispatcher.handle_callback("callback/job/start", start_params()),
        timeout=0.2,
    )
    assert result == {
        "accepted": True,
        "runner_handle": "callback:job-1:attempt:1",
    }
    await asyncio.wait_for(entered.wait(), timeout=0.2)
    assert resolver_calls == [("network", ("network.read",))]
    assert rpc_calls[0] == (
        "meta/jobs/progress",
        {
            "authority": AUTHORITY_1,
            "cursor": 1,
            "detail": "started",
            "observed_at_ms": 101,
        },
    )

    release.set()
    await dispatcher.wait_for_job_tasks()
    assert rpc_calls[-1] == (
        "meta/jobs/complete",
        {
            "authority": AUTHORITY_1,
            "completed_at_ms": rpc_calls[-1][1]["completed_at_ms"],
            "result_ref": "artifact:scan",
        },
    )
    assert "secret-current-attempt" not in repr(rpc_calls)


@pytest.mark.asyncio
async def test_duplicate_start_is_idempotent_and_newer_fence_supersedes_old() -> None:
    starts: list[int] = []
    releases = {7: asyncio.Event(), 8: asyncio.Event()}

    async def runner(context: DetachedJobContext) -> None:
        starts.append(context.authority.fence)
        try:
            await releases[context.authority.fence].wait()
        except asyncio.CancelledError:
            raise

    dispatcher = CallbackDispatcher()
    dispatcher.register_job_rpc(lambda _method, _params: asyncio.sleep(0))
    dispatcher.register_job_credential_resolver(
        lambda _profile, _scopes: {},
    )
    await register(dispatcher, runner)

    first = await dispatcher.handle_callback("callback/job/start", start_params())
    duplicate = await dispatcher.handle_callback("callback/job/start", start_params())
    await asyncio.sleep(0)
    assert first == duplicate
    assert starts == [7]

    second = await dispatcher.handle_callback(
        "callback/job/start",
        start_params(AUTHORITY_2, runner_handle="callback:job-1:attempt:2"),
    )
    await asyncio.sleep(0)
    assert second["accepted"] is True
    assert starts == [7, 8]

    stale = await dispatcher.handle_callback("callback/job/start", start_params())
    assert stale == {
        "accepted": False,
        "runner_handle": "callback:job-1:attempt:1",
    }
    releases[8].set()
    await dispatcher.wait_for_job_tasks()


@pytest.mark.asyncio
async def test_concurrent_duplicate_start_resolves_credentials_and_runs_once() -> None:
    resolver_entered = asyncio.Event()
    release_resolver = asyncio.Event()
    release_runner = asyncio.Event()
    resolver_calls = 0
    starts = 0

    async def resolve_credentials(
        _profile_name: str | None,
        _scopes: tuple[str, ...],
    ) -> dict[str, str]:
        nonlocal resolver_calls
        resolver_calls += 1
        resolver_entered.set()
        await release_resolver.wait()
        return {"token": "fresh"}

    async def runner(_context: DetachedJobContext) -> None:
        nonlocal starts
        starts += 1
        await release_runner.wait()

    dispatcher = CallbackDispatcher()
    dispatcher.register_job_rpc(lambda _method, _params: asyncio.sleep(0))
    dispatcher.register_job_credential_resolver(resolve_credentials)
    await register(dispatcher, runner)

    first = asyncio.create_task(
        dispatcher.handle_callback("callback/job/start", start_params())
    )
    await asyncio.wait_for(resolver_entered.wait(), timeout=0.2)
    duplicate = asyncio.create_task(
        dispatcher.handle_callback("callback/job/start", start_params())
    )
    await asyncio.sleep(0)
    release_resolver.set()
    first_result, duplicate_result = await asyncio.gather(first, duplicate)
    await asyncio.sleep(0)

    assert first_result == duplicate_result
    assert resolver_calls == 1
    assert starts == 1
    release_runner.set()
    await dispatcher.wait_for_job_tasks()


@pytest.mark.asyncio
async def test_reconcile_only_reports_exact_live_authority_and_never_starts_work() -> None:
    release = asyncio.Event()
    starts = 0
    rpc_calls: list[tuple[str, dict[str, Any]]] = []

    async def runner(_context: DetachedJobContext) -> None:
        nonlocal starts
        starts += 1
        await release.wait()

    async def rpc(method: str, params: dict[str, Any]) -> None:
        rpc_calls.append((method, params))

    dispatcher = CallbackDispatcher()
    dispatcher.register_job_rpc(rpc)
    dispatcher.register_job_credential_resolver(
        lambda _profile, _scopes: {},
    )
    await register(dispatcher, runner)
    await dispatcher.handle_callback("callback/job/start", start_params())
    await asyncio.sleep(0)

    result = await dispatcher.handle_callback(
        "callback/job/reconcile",
        {
            "attempts": [
                {
                    "authority": AUTHORITY_1,
                    "runner": {"name": "example.security_scan", "version": "1"},
                    "restart_class": "non_resumable",
                    "runner_handle": "callback:job-1:attempt:1",
                    "lease_expires_at_ms": 999,
                },
                {
                    "authority": {
                        "job_id": "job-missing",
                        "attempt_id": "attempt-missing",
                        "fence": 11,
                    },
                    "runner": {"name": "example.security_scan", "version": "1"},
                    "restart_class": "adoptable",
                    "runner_handle": "external:missing",
                    "lease_expires_at_ms": 999,
                },
            ]
        },
    )
    assert result == {"live_attempts": [AUTHORITY_1]}
    assert starts == 1
    heartbeat_method, heartbeat_params = rpc_calls[-1]
    assert heartbeat_method == "meta/jobs/heartbeat"
    assert heartbeat_params["authority"] == AUTHORITY_1
    assert heartbeat_params["lease_expires_at_ms"] > heartbeat_params["heartbeat_at_ms"]
    release.set()
    await dispatcher.wait_for_job_tasks()


@pytest.mark.asyncio
async def test_external_adoption_and_cancel_are_runner_owned_but_authority_exact() -> None:
    class AdoptableRunner:
        def __init__(self) -> None:
            self.cancelled: list[dict[str, Any]] = []

        async def run(self, _context: DetachedJobContext) -> None:
            raise AssertionError("reconcile must not start a fresh run")

        async def reconcile(self, attempt: dict[str, Any]) -> bool:
            return attempt["runner_handle"] == "external:still-live"

        async def cancel(self, authority: dict[str, Any]) -> None:
            self.cancelled.append(authority)

    runner = AdoptableRunner()
    dispatcher = CallbackDispatcher()
    dispatcher.register_job_rpc(lambda _method, _params: asyncio.sleep(0))
    await register(dispatcher, runner)

    offered = {
        "authority": AUTHORITY_1,
        "runner": {"name": "example.security_scan", "version": "1"},
        "restart_class": "adoptable",
        "runner_handle": "external:still-live",
        "lease_expires_at_ms": 999,
    }
    reconciled = await dispatcher.handle_callback(
        "callback/job/reconcile",
        {
            "attempts": [
                offered,
                {
                    "authority": {
                        "job_id": "job-replay",
                        "attempt_id": "attempt-replay",
                        "fence": 1,
                    },
                    "runner": {
                        "name": "example.security_scan",
                        "version": "1",
                    },
                    "restart_class": "replayable",
                    "runner_handle": "external:still-live",
                    "lease_expires_at_ms": 999,
                },
            ]
        },
    )
    assert reconciled == {"live_attempts": [AUTHORITY_1]}

    stale_cancel = await dispatcher.handle_callback(
        "callback/job/cancel",
        {"authority": AUTHORITY_2},
    )
    assert stale_cancel == {"accepted": False}

    exact_cancel = await dispatcher.handle_callback(
        "callback/job/cancel",
        {"authority": AUTHORITY_1},
    )
    assert exact_cancel == {"accepted": True}
    assert runner.cancelled == [AUTHORITY_1]


@pytest.mark.asyncio
async def test_credentials_are_re_resolved_per_attempt_and_profile_conflicts_fail() -> None:
    resolved: list[str] = []
    attempt_done = asyncio.Event()

    async def resolver(
        profile_name: str | None,
        _scopes: tuple[str, ...],
    ) -> dict[str, str]:
        token = f"{profile_name}-{len(resolved) + 1}"
        resolved.append(token)
        return {"token": token}

    async def runner(_context: DetachedJobContext) -> None:
        attempt_done.set()

    dispatcher = CallbackDispatcher()
    dispatcher.register_job_rpc(lambda _method, _params: asyncio.sleep(0))
    dispatcher.register_job_credential_resolver(resolver)
    await register(dispatcher, runner)
    await dispatcher.handle_callback("callback/job/start", start_params())
    await asyncio.wait_for(attempt_done.wait(), timeout=0.2)
    await dispatcher.wait_for_job_tasks()

    attempt_done.clear()
    await dispatcher.handle_callback(
        "callback/job/start",
        start_params(AUTHORITY_2, runner_handle="callback:job-1:attempt:2"),
    )
    await asyncio.wait_for(attempt_done.wait(), timeout=0.2)
    await dispatcher.wait_for_job_tasks()
    assert resolved == ["network-1", "network-2"]

    with pytest.raises(ValueError, match="already bound to profile 'network'"):
        await register(
            dispatcher,
            runner,
            profile_name="security",
            scope_id="build-2",
        )


@pytest.mark.asyncio
async def test_scoped_credential_resolution_failure_rejects_start_cleanly() -> None:
    dispatcher = CallbackDispatcher()
    dispatcher.register_job_rpc(lambda _method, _params: asyncio.sleep(0))
    await register(dispatcher, lambda _context: None)

    assert await dispatcher.handle_callback(
        "callback/job/start",
        start_params(),
    ) == {
        "accepted": False,
        "runner_handle": "callback:job-1:attempt:1",
    }
