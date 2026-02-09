"""Meerkat Python SDK — communicate with the Meerkat agent runtime via JSON-RPC."""

from .client import MeerkatClient
from .errors import (
    CapabilityUnavailableError,
    MeerkatError,
    SessionNotFoundError,
    SkillNotFoundError,
)
from .generated.types import (
    CONTRACT_VERSION,
    CapabilitiesResponse,
    CapabilityEntry,
    WireEvent,
    WireRunResult,
    WireUsage,
)

__all__ = [
    "MeerkatClient",
    "MeerkatError",
    "CapabilityUnavailableError",
    "SessionNotFoundError",
    "SkillNotFoundError",
    "CONTRACT_VERSION",
    "WireUsage",
    "WireRunResult",
    "WireEvent",
    "CapabilitiesResponse",
    "CapabilityEntry",
]
