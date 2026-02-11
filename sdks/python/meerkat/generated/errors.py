"""Generated error types for Meerkat SDK.

Re-exports from meerkat.errors to avoid duplicate class hierarchies.
"""

from meerkat.errors import (
    CapabilityUnavailableError,
    MeerkatError,
    SessionNotFoundError,
    SkillNotFoundError,
)

__all__ = [
    "MeerkatError",
    "CapabilityUnavailableError",
    "SessionNotFoundError",
    "SkillNotFoundError",
]
