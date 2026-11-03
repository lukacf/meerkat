"""Memory backend configuration for Meta runtime.

The operational memory ledger (``meta/memory/*``) is persisted by the
gateway as local JSON under ``persistent_state``, with an optional HTTP
health-check gate. ``local_json()`` configures that backend, the only one
the gateway accepts.
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Any


@dataclass(frozen=True)
class LocalJsonMemoryConfig:
    """Local-JSON operational ledger backend with an optional health gate.

    When ``health_check_endpoint`` is set, the gateway health-checks
    ``GET <endpoint>/v1/health`` before every ledger read/write.
    """

    health_check_endpoint: str | None = None

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {"backend": "local_json"}
        if self.health_check_endpoint is not None:
            result["health_check_endpoint"] = self.health_check_endpoint
        return result


def local_json(health_check_endpoint: str | None = None) -> LocalJsonMemoryConfig:
    return LocalJsonMemoryConfig(health_check_endpoint=health_check_endpoint)
