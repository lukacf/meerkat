"""Detached callback-job contracts for the Meta host SDK.

The Meerkat job machine owns lifecycle, fences, retries, loss, and terminality.
This module is a mechanical host shell: it accepts an already-committed
attempt, runs the registered host runner outside the callback, and reports
mutations with the exact authority supplied by Meerkat.
"""
from __future__ import annotations

import asyncio
from dataclasses import dataclass
import time
from typing import Any, Awaitable, Callable, Literal, Mapping


RestartClass = Literal[
    "adoptable",
    "checkpoint_resumable",
    "replayable",
    "non_resumable",
]
IdempotencyScope = Literal[
    "tool_call",
    "interaction_and_arguments",
    "host_semantic_key",
]


@dataclass(frozen=True)
class DetachedJobAuthority:
    job_id: str
    attempt_id: str
    fence: int

    @classmethod
    def from_dict(cls, value: Mapping[str, Any]) -> DetachedJobAuthority:
        job_id = value.get("job_id")
        attempt_id = value.get("attempt_id")
        fence = value.get("fence")
        if not isinstance(job_id, str) or not job_id:
            raise ValueError("job authority requires a non-empty job_id")
        if not isinstance(attempt_id, str) or not attempt_id:
            raise ValueError("job authority requires a non-empty attempt_id")
        if isinstance(fence, bool) or not isinstance(fence, int) or fence < 1:
            raise ValueError("job authority requires a positive integer fence")
        return cls(job_id=job_id, attempt_id=attempt_id, fence=fence)

    def to_dict(self) -> dict[str, Any]:
        return {
            "job_id": self.job_id,
            "attempt_id": self.attempt_id,
            "fence": self.fence,
        }


@dataclass(frozen=True)
class DetachedJobResult:
    """A host runner's terminal result reference.

    Result bytes belong in the canonical blob/artifact store. This object
    carries only the durable non-secret reference.
    """

    result_ref: str | None = None


DetachedJobRpc = Callable[[str, dict[str, Any]], Awaitable[Any]]
CredentialResolver = Callable[
    [str | None, tuple[str, ...]],
    Mapping[str, Any] | Awaitable[Mapping[str, Any]],
]


@dataclass(frozen=True)
class DetachedJobExecution:
    """Private execution metadata plus the host-side runner implementation."""

    runner: str
    version: str
    restart_class: RestartClass
    idempotency_scope: IdempotencyScope
    submission_timeout_ms: int
    credential_scopes: tuple[str, ...] = ()
    handler: Any = None

    def __post_init__(self) -> None:
        if not isinstance(self.runner, str) or not self.runner.strip():
            raise ValueError("detached runner must be a non-empty string")
        if not isinstance(self.version, str) or not self.version.strip():
            raise ValueError("detached runner version must be a non-empty string")
        if self.restart_class not in {
            "adoptable",
            "checkpoint_resumable",
            "replayable",
            "non_resumable",
        }:
            raise ValueError(f"unsupported restart_class: {self.restart_class!r}")
        if self.idempotency_scope not in {
            "tool_call",
            "interaction_and_arguments",
            "host_semantic_key",
        }:
            raise ValueError(
                f"unsupported idempotency_scope: {self.idempotency_scope!r}"
            )
        if (
            isinstance(self.submission_timeout_ms, bool)
            or not isinstance(self.submission_timeout_ms, int)
            or not 0 < self.submission_timeout_ms <= 120_000
        ):
            raise ValueError(
                "submission_timeout_ms must be an integer in the public "
                "1..=120000ms callback window"
            )
        if self.handler is None or not (
            callable(self.handler) or callable(getattr(self.handler, "run", None))
        ):
            raise TypeError("detached execution requires a callable handler or runner.run")
        normalized_scopes: list[str] = []
        for scope in self.credential_scopes:
            if not isinstance(scope, str) or not scope.strip():
                raise ValueError("credential scopes must be non-empty strings")
            normalized_scopes.append(scope)
        object.__setattr__(self, "credential_scopes", tuple(normalized_scopes))

    @property
    def runner_key(self) -> tuple[str, str]:
        return (self.runner, self.version)

    def to_wire(self) -> dict[str, Any]:
        result: dict[str, Any] = {
            "mode": "detached",
            "runner": {"name": self.runner, "version": self.version},
            "restart_class": self.restart_class,
            "idempotency_scope": self.idempotency_scope,
            "submission_timeout_ms": self.submission_timeout_ms,
        }
        if self.credential_scopes:
            result["credential_scopes"] = list(self.credential_scopes)
        return result


class _DetachedJobReporter:
    def __init__(self, authority: DetachedJobAuthority, rpc: DetachedJobRpc) -> None:
        self._authority = authority
        self._rpc = rpc
        self._terminal = False

    @property
    def terminal(self) -> bool:
        return self._terminal

    async def _send(self, method: str, params: dict[str, Any]) -> Any:
        return await self._rpc(
            method,
            {"authority": self._authority.to_dict(), **params},
        )

    async def heartbeat(
        self,
        *,
        heartbeat_at_ms: int | None = None,
        lease_expires_at_ms: int,
    ) -> Any:
        return await self._send(
            "meta/jobs/heartbeat",
            {
                "heartbeat_at_ms": heartbeat_at_ms or _unix_time_ms(),
                "lease_expires_at_ms": lease_expires_at_ms,
            },
        )

    async def progress(
        self,
        cursor: int,
        detail: str,
        *,
        observed_at_ms: int | None = None,
    ) -> Any:
        return await self._send(
            "meta/jobs/progress",
            {
                "cursor": cursor,
                "detail": detail,
                "observed_at_ms": observed_at_ms or _unix_time_ms(),
            },
        )

    async def checkpoint(
        self,
        checkpoint_ref: str,
        *,
        observed_at_ms: int | None = None,
    ) -> Any:
        return await self._send(
            "meta/jobs/checkpoint",
            {
                "checkpoint_ref": checkpoint_ref,
                "observed_at_ms": observed_at_ms or _unix_time_ms(),
            },
        )

    async def complete(
        self,
        result_ref: str | None,
        *,
        completed_at_ms: int | None = None,
    ) -> Any:
        if self._terminal:
            return None
        params: dict[str, Any] = {
            "completed_at_ms": completed_at_ms or _unix_time_ms(),
        }
        if result_ref is not None:
            params["result_ref"] = result_ref
        result = await self._send("meta/jobs/complete", params)
        self._terminal = True
        return result

    async def fail(
        self,
        code: str,
        *,
        detail_ref: str | None = None,
        failed_at_ms: int | None = None,
    ) -> Any:
        if self._terminal:
            return None
        params: dict[str, Any] = {
            "failed_at_ms": failed_at_ms or _unix_time_ms(),
            "code": code,
        }
        if detail_ref is not None:
            params["detail_ref"] = detail_ref
        result = await self._send("meta/jobs/fail", params)
        self._terminal = True
        return result

    async def cancel_ack(self, *, acknowledged_at_ms: int | None = None) -> Any:
        if self._terminal:
            return None
        result = await self._send(
            "meta/jobs/cancel_ack",
            {"acknowledged_at_ms": acknowledged_at_ms or _unix_time_ms()},
        )
        self._terminal = True
        return result


class DetachedJobContext:
    """Attempt-local host context.

    Credentials are freshly resolved ephemeral values. They are never included
    by any reporter method and must not be retained by the runner.
    """

    def __init__(
        self,
        *,
        authority: DetachedJobAuthority,
        runner_handle: str,
        arguments: Any,
        credentials: Mapping[str, Any],
        resume_checkpoint: str | None,
        cancellation: asyncio.Event,
        reporter: _DetachedJobReporter,
    ) -> None:
        self.authority = authority
        self.runner_handle = runner_handle
        self.arguments = arguments
        self.credentials = dict(credentials)
        self.resume_checkpoint = resume_checkpoint
        self.cancellation = cancellation
        self._reporter = reporter

    @property
    def cancelled(self) -> bool:
        return self.cancellation.is_set()

    async def heartbeat(
        self,
        *,
        lease_expires_at_ms: int,
        heartbeat_at_ms: int | None = None,
    ) -> Any:
        return await self._reporter.heartbeat(
            heartbeat_at_ms=heartbeat_at_ms,
            lease_expires_at_ms=lease_expires_at_ms,
        )

    async def progress(
        self,
        cursor: int,
        detail: str,
        *,
        observed_at_ms: int | None = None,
    ) -> Any:
        return await self._reporter.progress(
            cursor,
            detail,
            observed_at_ms=observed_at_ms,
        )

    async def checkpoint(
        self,
        checkpoint_ref: str,
        *,
        observed_at_ms: int | None = None,
    ) -> Any:
        return await self._reporter.checkpoint(
            checkpoint_ref,
            observed_at_ms=observed_at_ms,
        )


def _unix_time_ms() -> int:
    return time.time_ns() // 1_000_000
