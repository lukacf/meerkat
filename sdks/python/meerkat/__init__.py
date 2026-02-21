"""Meerkat Python SDK — communicate with the Meerkat agent runtime via JSON-RPC."""

from .capabilities import CapabilityChecker
from .client import MeerkatClient
from .errors import (
    CapabilityUnavailableError,
    MeerkatError,
    SessionNotFoundError,
    SkillNotFoundError,
)
from .skills import SkillHelper, SkillKey
from .streaming import StreamingTurn
from .types import (
    CONTRACT_VERSION,
    CapabilitiesResponse,
    CapabilityEntry,
    WireEvent,
    WireRunResult,
    WireUsage,
)

__all__ = [
    "MeerkatClient",
    "CapabilityChecker",
    "StreamingTurn",
    "SkillHelper",
    "SkillKey",
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
