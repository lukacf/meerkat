"""Meerkat Python SDK — communicate with the Meerkat agent runtime.

Quick start::

    import asyncio
    from meerkat import MeerkatClient, TextDelta

    async def main():
        async with MeerkatClient() as client:
            session = await client.create_session("Hello, Meerkat!")
            print(session.text)

            async with session.stream("Tell me a joke") as events:
                async for event in events:
                    match event:
                        case TextDelta(delta=chunk):
                            print(chunk, end="", flush=True)
                print()

    asyncio.run(main())
"""

# Core client and session
from .client import MeerkatClient
from .session import Session
from .streaming import EventStream

# Domain types (clean, Wire-free public names)
from .types import (
    CONTRACT_VERSION,
    Capability,
    RunResult,
    SchemaWarning,
    SessionInfo,
    SkillQuarantineDiagnostic,
    SkillRuntimeDiagnostics,
    SkillKey,
    SkillRef,
    SourceHealthSnapshot,
    Usage,
)

# Error hierarchy
from .errors import (
    CapabilityUnavailableError,
    MeerkatError,
    SessionNotFoundError,
    SkillNotFoundError,
)

# Typed event hierarchy — every event variant is a frozen dataclass
from .events import (
    BudgetWarning,
    CompactionCompleted,
    CompactionFailed,
    CompactionStarted,
    Event,
    HookCompleted,
    HookDenied,
    HookFailed,
    HookPatchPublished,
    HookRewriteApplied,
    HookStarted,
    InteractionComplete,
    InteractionFailed,
    Retrying,
    RunCompleted,
    RunFailed,
    RunStarted,
    SkillResolutionFailed,
    SkillsResolved,
    StreamTruncated,
    TextComplete,
    TextDelta,
    ToolCallRequested,
    ToolExecutionCompleted,
    ToolExecutionStarted,
    ToolExecutionTimedOut,
    ToolResultReceived,
    TurnCompleted,
    TurnStarted,
    UnknownEvent,
    parse_event,
)

__all__ = [
    # Client & session
    "MeerkatClient",
    "Session",
    "EventStream",
    # Types
    "CONTRACT_VERSION",
    "Capability",
    "RunResult",
    "SchemaWarning",
    "SessionInfo",
    "SkillQuarantineDiagnostic",
    "SkillRuntimeDiagnostics",
    "SkillKey",
    "SkillRef",
    "SourceHealthSnapshot",
    "Usage",
    # Errors
    "MeerkatError",
    "CapabilityUnavailableError",
    "SessionNotFoundError",
    "SkillNotFoundError",
    # Events (base + all variants)
    "Event",
    "RunStarted",
    "RunCompleted",
    "RunFailed",
    "TurnStarted",
    "TextDelta",
    "TextComplete",
    "ToolCallRequested",
    "ToolResultReceived",
    "TurnCompleted",
    "ToolExecutionStarted",
    "ToolExecutionCompleted",
    "ToolExecutionTimedOut",
    "CompactionStarted",
    "CompactionCompleted",
    "CompactionFailed",
    "BudgetWarning",
    "Retrying",
    "HookStarted",
    "HookCompleted",
    "HookFailed",
    "HookDenied",
    "HookRewriteApplied",
    "HookPatchPublished",
    "SkillsResolved",
    "SkillResolutionFailed",
    "InteractionComplete",
    "InteractionFailed",
    "StreamTruncated",
    "UnknownEvent",
    "parse_event",
]
