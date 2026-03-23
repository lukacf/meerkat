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
from .mob import Mob
from .session import DeferredSession, Session
from .streaming import EventStream, EventSubscription

# Domain types (clean, Wire-free public names)
from .types import (
    CONTRACT_VERSION,
    AttributedEvent,
    Capability,
    EventEnvelope,
    InputListParams,
    InputListResult,
    McpAddParams,
    McpLiveOpResponse,
    McpReloadParams,
    McpRemoveParams,
    RunResult,
    RuntimeAcceptParams,
    RuntimeAcceptResult,
    RuntimeResetParams,
    RuntimeResetResult,
    RuntimeRetireParams,
    RuntimeRetireResult,
    RuntimeStateParams,
    RuntimeStateResult,
    SchemaWarning,
    SessionAssistantBlock,
    SessionHistory,
    SessionInfo,
    SessionMessage,
    SessionToolCall,
    SessionToolResult,
    SkillQuarantineDiagnostic,
    SkillRuntimeDiagnostics,
    SkillKey,
    SkillRef,
    SourceHealthSnapshot,
    WireInputState,
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
    ScopedEvent,
    SkillResolutionFailed,
    SkillsResolved,
    StreamTruncated,
    TextComplete,
    TextDelta,
    ToolConfigChanged,
    ToolConfigChangedPayload,
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
    "DeferredSession",
    "Session",
    "Mob",
    "EventStream",
    "EventSubscription",
    # Types
    "CONTRACT_VERSION",
    "AttributedEvent",
    "Capability",
    "EventEnvelope",
    "InputListParams",
    "InputListResult",
    "McpAddParams",
    "McpRemoveParams",
    "McpReloadParams",
    "McpLiveOpResponse",
    "RuntimeAcceptParams",
    "RuntimeAcceptResult",
    "RuntimeResetParams",
    "RuntimeResetResult",
    "RuntimeRetireParams",
    "RuntimeRetireResult",
    "RuntimeStateParams",
    "RuntimeStateResult",
    "RunResult",
    "SchemaWarning",
    "SessionAssistantBlock",
    "SessionHistory",
    "SessionInfo",
    "SessionMessage",
    "SessionToolCall",
    "SessionToolResult",
    "SkillQuarantineDiagnostic",
    "SkillRuntimeDiagnostics",
    "SkillKey",
    "SkillRef",
    "SourceHealthSnapshot",
    "WireInputState",
    "Usage",
    # Errors
    "MeerkatError",
    "CapabilityUnavailableError",
    "SessionNotFoundError",
    "SkillNotFoundError",
    # Events (base + all variants)
    "Event",
    "RunStarted",
    "ScopedEvent",
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
    "ToolConfigChanged",
    "ToolConfigChangedPayload",
    "UnknownEvent",
    "parse_event",
]
