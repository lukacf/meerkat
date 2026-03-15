---- MODULE model ----
EXTENDS Naturals, TLC

\* Abstract, architecture-level model of TurnExecutionMachine.
\* This models one active run at a time and the internal LLM/tool/boundary loop.

CONSTANTS
    RunIds,
    NoRun,
    MaxToolCalls,
    MaxBoundaries

ASSUME NoRun \notin RunIds
ASSUME MaxToolCalls \in Nat
ASSUME MaxBoundaries \in Nat

ExecStates ==
    {"ready", "applying_primitive", "calling_llm", "waiting_for_ops",
     "draining_boundary", "error_recovery", "cancelling",
     "completed", "failed", "cancelled"}

PrimitiveKinds ==
    {"none", "conversation_turn", "immediate_append", "immediate_context"}

TerminalOutcomes == {"none", "completed", "failed", "cancelled"}

VARIABLES
    execState,
    activeRun,
    primitiveKind,
    toolCallsPending,
    boundaryCount,
    terminalOutcome

vars ==
    << execState, activeRun, primitiveKind,
       toolCallsPending, boundaryCount, terminalOutcome >>

Init ==
    /\ execState = "ready"
    /\ activeRun = NoRun
    /\ primitiveKind = "none"
    /\ toolCallsPending = 0
    /\ boundaryCount = 0
    /\ terminalOutcome = "none"

StartConversationRun(r) ==
    /\ r \in RunIds
    /\ execState = "ready"
    /\ activeRun = NoRun
    /\ execState' = "applying_primitive"
    /\ activeRun' = r
    /\ primitiveKind' = "conversation_turn"
    /\ toolCallsPending' = 0
    /\ boundaryCount' = 0
    /\ terminalOutcome' = "none"

StartImmediateAppend(r) ==
    /\ r \in RunIds
    /\ execState = "ready"
    /\ activeRun = NoRun
    /\ execState' = "applying_primitive"
    /\ activeRun' = r
    /\ primitiveKind' = "immediate_append"
    /\ toolCallsPending' = 0
    /\ boundaryCount' = 0
    /\ terminalOutcome' = "none"

StartImmediateContext(r) ==
    /\ r \in RunIds
    /\ execState = "ready"
    /\ activeRun = NoRun
    /\ execState' = "applying_primitive"
    /\ activeRun' = r
    /\ primitiveKind' = "immediate_context"
    /\ toolCallsPending' = 0
    /\ boundaryCount' = 0
    /\ terminalOutcome' = "none"

PrimitiveAppliedConversation ==
    /\ execState = "applying_primitive"
    /\ primitiveKind = "conversation_turn"
    /\ activeRun # NoRun
    /\ execState' = "calling_llm"
    /\ UNCHANGED << activeRun, primitiveKind, toolCallsPending, boundaryCount, terminalOutcome >>

PrimitiveAppliedImmediate ==
    /\ execState = "applying_primitive"
    /\ primitiveKind \in {"immediate_append", "immediate_context"}
    /\ activeRun # NoRun
    /\ boundaryCount < MaxBoundaries
    /\ execState' = "completed"
    /\ boundaryCount' = boundaryCount + 1
    /\ terminalOutcome' = "completed"
    /\ UNCHANGED << activeRun, primitiveKind, toolCallsPending >>

LlmReturnedToolCalls(n) ==
    /\ execState = "calling_llm"
    /\ activeRun # NoRun
    /\ n \in 1..MaxToolCalls
    /\ execState' = "waiting_for_ops"
    /\ toolCallsPending' = n
    /\ UNCHANGED << activeRun, primitiveKind, boundaryCount, terminalOutcome >>

ToolCallsResolved ==
    /\ execState = "waiting_for_ops"
    /\ activeRun # NoRun
    /\ toolCallsPending > 0
    /\ boundaryCount < MaxBoundaries
    /\ execState' = "draining_boundary"
    /\ toolCallsPending' = 0
    /\ boundaryCount' = boundaryCount + 1
    /\ UNCHANGED << activeRun, primitiveKind, terminalOutcome >>

LlmReturnedTerminal ==
    /\ execState = "calling_llm"
    /\ activeRun # NoRun
    /\ boundaryCount < MaxBoundaries
    /\ execState' = "draining_boundary"
    /\ boundaryCount' = boundaryCount + 1
    /\ UNCHANGED << activeRun, primitiveKind, toolCallsPending, terminalOutcome >>

BoundaryContinue ==
    /\ execState = "draining_boundary"
    /\ activeRun # NoRun
    /\ primitiveKind = "conversation_turn"
    /\ execState' = "calling_llm"
    /\ UNCHANGED << activeRun, primitiveKind, toolCallsPending, boundaryCount, terminalOutcome >>

BoundaryComplete ==
    /\ execState = "draining_boundary"
    /\ activeRun # NoRun
    /\ execState' = "completed"
    /\ terminalOutcome' = "completed"
    /\ UNCHANGED << activeRun, primitiveKind, toolCallsPending, boundaryCount >>

RecoverableFailure ==
    /\ execState \in {"calling_llm", "waiting_for_ops", "draining_boundary"}
    /\ activeRun # NoRun
    /\ execState' = "error_recovery"
    /\ UNCHANGED << activeRun, primitiveKind, toolCallsPending, boundaryCount, terminalOutcome >>

RetryRequested ==
    /\ execState = "error_recovery"
    /\ activeRun # NoRun
    /\ execState' = "calling_llm"
    /\ UNCHANGED << activeRun, primitiveKind, toolCallsPending, boundaryCount, terminalOutcome >>

FatalFailure ==
    /\ execState \in {"applying_primitive", "calling_llm", "waiting_for_ops",
                      "draining_boundary", "error_recovery"}
    /\ activeRun # NoRun
    /\ execState' = "failed"
    /\ terminalOutcome' = "failed"
    /\ UNCHANGED << activeRun, primitiveKind, toolCallsPending, boundaryCount >>

CancelRequested ==
    /\ execState \in {"applying_primitive", "calling_llm", "waiting_for_ops",
                      "draining_boundary", "error_recovery"}
    /\ activeRun # NoRun
    /\ execState' = "cancelling"
    /\ UNCHANGED << activeRun, primitiveKind, toolCallsPending, boundaryCount, terminalOutcome >>

CancellationObserved ==
    /\ execState = "cancelling"
    /\ activeRun # NoRun
    /\ execState' = "cancelled"
    /\ terminalOutcome' = "cancelled"
    /\ UNCHANGED << activeRun, primitiveKind, toolCallsPending, boundaryCount >>

AcknowledgeTerminal ==
    /\ execState \in {"completed", "failed", "cancelled"}
    /\ activeRun # NoRun
    /\ execState' = "ready"
    /\ activeRun' = NoRun
    /\ primitiveKind' = "none"
    /\ toolCallsPending' = 0
    /\ boundaryCount' = 0
    /\ terminalOutcome' = "none"

Next ==
    \/ \E r \in RunIds : StartConversationRun(r)
    \/ \E r \in RunIds : StartImmediateAppend(r)
    \/ \E r \in RunIds : StartImmediateContext(r)
    \/ PrimitiveAppliedConversation
    \/ PrimitiveAppliedImmediate
    \/ \E n \in 1..MaxToolCalls : LlmReturnedToolCalls(n)
    \/ ToolCallsResolved
    \/ LlmReturnedTerminal
    \/ BoundaryContinue
    \/ BoundaryComplete
    \/ RecoverableFailure
    \/ RetryRequested
    \/ FatalFailure
    \/ CancelRequested
    \/ CancellationObserved
    \/ AcknowledgeTerminal

ReadyHasNoRun ==
    (execState = "ready") <=> (activeRun = NoRun)

WaitingHasPendingTools ==
    execState = "waiting_for_ops" => toolCallsPending > 0

ImmediateNeverEntersLlmOrOps ==
    primitiveKind \in {"immediate_append", "immediate_context"} =>
        execState \notin {"calling_llm", "waiting_for_ops", "error_recovery"}

TerminalOutcomeMatchesState ==
    /\ (execState = "completed") => terminalOutcome = "completed"
    /\ (execState = "failed") => terminalOutcome = "failed"
    /\ (execState = "cancelled") => terminalOutcome = "cancelled"

CompletedRequiresBoundary ==
    execState = "completed" => boundaryCount > 0

Spec == Init /\ [][Next]_vars

THEOREM Spec => []ReadyHasNoRun
THEOREM Spec => []WaitingHasPendingTools
THEOREM Spec => []ImmediateNeverEntersLlmOrOps
THEOREM Spec => []TerminalOutcomeMatchesState
THEOREM Spec => []CompletedRequiresBoundary

=============================================================================
