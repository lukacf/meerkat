"""Meerkat SDK error types.

Single source of truth: the generated error hierarchy. This module
re-exports the generated classes so hand-written and generated code raise
the same exception types (K21 — the generated fail-closed parsers raise
``meerkat.generated.errors.MeerkatError``).
"""

from .generated.errors import (  # noqa: F401
    CapabilityUnavailableError as CapabilityUnavailableError,
    MeerkatError as MeerkatError,
    SessionNotFoundError as SessionNotFoundError,
    SkillNotFoundError as SkillNotFoundError,
)
