"""Event-log durability declaration for Meta runtime.

The gateway ingests operational events only when
``runtime_options.event_log`` is configured; the absence of the key means
no ingestion at all (the honest silent case). The two wire-supported
storage kinds are both *declared ephemeral* choices:

- :func:`memory` — a bounded, queryable in-process store
  (``{"storage": "memory"}``); serves ``meta/query_events``.
- :func:`null` — events are explicitly dropped (``{"storage": "null"}``);
  queries return empty.

Durable event-log backends are embedder-only today: the Rust
``UnifiedRuntimeBuilder`` accepts any ``EventLogStore`` implementation, but
the gateway wire supports only the declarations above and rejects anything
else at startup (``unsupported runtime_options.event_log.storage``). The
resolved choice is visible in the storage census (``meta/status`` →
``storage.slots``).
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Any


@dataclass(frozen=True)
class MemoryEventLogConfig:
    """Bounded queryable in-process event store (declared ephemeral)."""

    batch_size: int | None = None
    flush_interval_ms: int | None = None

    def __post_init__(self) -> None:
        # A zero interval panics the gateway's ingestion task
        # (`tokio::time::interval` requires a non-zero period), and the
        # wire only accepts positive integers.
        if self.flush_interval_ms is not None and (
            not isinstance(self.flush_interval_ms, int)
            or isinstance(self.flush_interval_ms, bool)
            or self.flush_interval_ms <= 0
        ):
            raise ValueError(
                "flush_interval_ms must be a positive integer "
                f"(got {self.flush_interval_ms!r})"
            )

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {"storage": "memory"}
        if self.batch_size is not None:
            result["batch_size"] = self.batch_size
        if self.flush_interval_ms is not None:
            result["flush_interval_ms"] = self.flush_interval_ms
        return result


@dataclass(frozen=True)
class NullEventLogConfig:
    """Events are explicitly dropped (declared ephemeral, queries empty)."""

    def to_dict(self) -> dict[str, Any]:
        return {"storage": "null"}


def memory(
    batch_size: int | None = None,
    flush_interval_ms: int | None = None,
) -> MemoryEventLogConfig:
    """Declare a bounded queryable in-process event store."""
    return MemoryEventLogConfig(
        batch_size=batch_size,
        flush_interval_ms=flush_interval_ms,
    )


def null() -> NullEventLogConfig:
    """Declare that operational events are dropped."""
    return NullEventLogConfig()
