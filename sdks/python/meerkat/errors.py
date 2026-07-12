"""Meerkat SDK error types.

Single source of truth: the generated error hierarchy. This module
re-exports the generated classes so hand-written and generated code raise
the same exception types (K21 — the generated fail-closed parsers raise
``meerkat.generated.errors.MeerkatError``).
"""

from .generated.errors import (  # noqa: F401
    CapabilityUnavailableError as CapabilityUnavailableError,
    HostUnavailableError as HostUnavailableError,
    MULTI_HOST_JSON_RPC_ERROR_CODES as MULTI_HOST_JSON_RPC_ERROR_CODES,
    MeerkatError as MeerkatError,
    MultiHostErrorCode as MultiHostErrorCode,
    ScopeDeniedError as ScopeDeniedError,
    SessionNotFoundError as SessionNotFoundError,
    SkillNotFoundError as SkillNotFoundError,
    StaleCursorError as StaleCursorError,
    StaleFenceError as StaleFenceError,
    WireHostUnavailableDetail as WireHostUnavailableDetail,
    WireScopeDeniedDetail as WireScopeDeniedDetail,
    WireStaleCursorDetail as WireStaleCursorDetail,
    WireStaleFenceDetail as WireStaleFenceDetail,
    meerkat_error_from_jsonrpc_code as meerkat_error_from_jsonrpc_code,
    meerkat_error_from_semantic_code as meerkat_error_from_semantic_code,
)
