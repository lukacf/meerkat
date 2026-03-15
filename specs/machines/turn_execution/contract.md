# TurnExecutionMachine

Status: normative `0.5` machine contract, first formal-spec draft

## Purpose

`TurnExecutionMachine` owns the execution of one runtime-admitted run at a
time.

It is the machine inside runtime control that turns one `RunPrimitive` plus the
current session value into `RunEvent` effects and an updated session.

It owns:

- application of one admitted `RunPrimitive`
- the internal loop over LLM/tool/boundary execution
- cooperative suspension while waiting for tool completions
- retry/cancel execution transitions inside the run
- terminal completion/failure/cancel for one active run

It is **not** the owner of:

- ordinary runtime admission
- queue ordering
- host idle waiting
- control-plane precedence
- peer comms inbox draining as a separate outer loop

Important structural rule:

- the host-mode idle/wake/drain loop is not part of `TurnExecutionMachine`
- `0.5` narrows `Agent` toward the execution core inside this machine and moves
  idle/runtime coordination upward into `RuntimeControlMachine`

## Scope Boundary

This machine begins when runtime has:

- selected a `RunId`
- selected one `RunPrimitive`
- decided that this run should execute now

It ends when the run reaches one terminal execution outcome:

- completed
- failed
- cancelled

The machine may require multiple inner LLM/boundary cycles before it reaches
that terminal outcome, but it still owns exactly one active run at a time.

## Authoritative State Model

For one runtime instance, the machine state is the tuple:

- `execution_state: TurnExecutionState`
- `active_run: Option<RunId>`
- `primitive_kind: TurnPrimitiveKind`
- `tool_calls_pending: u32`
- `boundary_count: u32`
- `terminal_outcome: TurnTerminalOutcome`

`TurnExecutionState` is the closed state set:

- `Ready`
- `ApplyingPrimitive`
- `CallingLlm`
- `WaitingForOps`
- `DrainingBoundary`
- `ErrorRecovery`
- `Cancelling`
- `Completed`
- `Failed`
- `Cancelled`

Terminal states:

- `Completed`
- `Failed`
- `Cancelled`

`TurnPrimitiveKind` is:

- `None`
- `ConversationTurn`
- `ImmediateAppend`
- `ImmediateContextAppend`

`TurnTerminalOutcome` is:

- `None`
- `Completed`
- `Failed`
- `Cancelled`

## Input Alphabet

The closed external input/command alphabet for this machine is:

- `StartConversationRun(run_id)`
- `StartImmediateAppend(run_id)`
- `StartImmediateContext(run_id)`
- `PrimitiveApplied(run_id)`
- `LlmReturnedToolCalls(run_id, tool_count)`
- `LlmReturnedTerminal(run_id)`
- `ToolCallsResolved(run_id)`
- `BoundaryContinue(run_id)`
- `BoundaryComplete(run_id)`
- `RecoverableFailure(run_id)`
- `FatalFailure(run_id)`
- `RetryRequested(run_id)`
- `CancelRequested(run_id)`
- `CancellationObserved(run_id)`
- `AcknowledgeTerminal(run_id)`

Notes:

- the `Start*` family is the normalized runtime-to-turn-execution handoff
- immediate append/context primitives are modeled explicitly because they do
  not require an LLM/tool cycle
- `BoundaryContinue` covers cases where boundary application is complete but
  execution must re-enter another LLM cycle within the same run
  (for example tool-loop continuation or structured-output retry)

## Effect Family

The closed machine-boundary effect family is the canonical `RunEvent` family:

- `RunStarted(run_id)`
- `BoundaryApplied(run_id)`
- `RunCompleted(run_id)`
- `RunFailed(run_id)`
- `RunCancelled(run_id)`

Implementation note:

- the current Rust implementation realizes these through `RunEvent`,
  `RunBoundaryReceipt`, `RunResult`, session mutation, checkpoint/save logic,
  and typed control calls
- `0.5` treats `RunEvent` as the authoritative cross-machine effect family

## Transition Relation

### Run start

1. `StartConversationRun`

Preconditions:

- `execution_state = Ready`
- `active_run = None`

State updates:

- `execution_state := ApplyingPrimitive`
- `active_run := run_id`
- `primitive_kind := ConversationTurn`
- `tool_calls_pending := 0`
- `boundary_count := 0`
- `terminal_outcome := None`

Effect:

- `RunStarted(run_id)`

2. `StartImmediateAppend`

Same as above, except:

- `primitive_kind := ImmediateAppend`

3. `StartImmediateContext`

Same as above, except:

- `primitive_kind := ImmediateContextAppend`

### Primitive application

4. `PrimitiveApplied` for `ConversationTurn`

Preconditions:

- `execution_state = ApplyingPrimitive`
- `primitive_kind = ConversationTurn`
- `active_run = run_id`

State updates:

- `execution_state := CallingLlm`

5. `PrimitiveApplied` for immediate primitives

Preconditions:

- `execution_state = ApplyingPrimitive`
- `primitive_kind ∈ {ImmediateAppend, ImmediateContextAppend}`
- `active_run = run_id`

State updates:

- `execution_state := Completed`
- `boundary_count := boundary_count + 1`
- `terminal_outcome := Completed`

Effects:

- `BoundaryApplied(run_id)`
- `RunCompleted(run_id)`

### LLM/tool loop

6. `LlmReturnedToolCalls`

Preconditions:

- `execution_state = CallingLlm`
- `active_run = run_id`
- `tool_count > 0`

State updates:

- `execution_state := WaitingForOps`
- `tool_calls_pending := tool_count`

7. `ToolCallsResolved`

Preconditions:

- `execution_state = WaitingForOps`
- `active_run = run_id`
- `tool_calls_pending > 0`

State updates:

- `execution_state := DrainingBoundary`
- `tool_calls_pending := 0`
- `boundary_count := boundary_count + 1`

Effect:

- `BoundaryApplied(run_id)`

8. `LlmReturnedTerminal`

Preconditions:

- `execution_state = CallingLlm`
- `active_run = run_id`

State updates:

- `execution_state := DrainingBoundary`
- `boundary_count := boundary_count + 1`

Effect:

- `BoundaryApplied(run_id)`

9. `BoundaryContinue`

Preconditions:

- `execution_state = DrainingBoundary`
- `active_run = run_id`
- `primitive_kind = ConversationTurn`

State updates:

- `execution_state := CallingLlm`

10. `BoundaryComplete`

Preconditions:

- `execution_state = DrainingBoundary`
- `active_run = run_id`

State updates:

- `execution_state := Completed`
- `terminal_outcome := Completed`

Effect:

- `RunCompleted(run_id)`

### Failure and recovery

11. `RecoverableFailure`

Preconditions:

- `execution_state ∈ {CallingLlm, WaitingForOps, DrainingBoundary}`
- `active_run = run_id`

State updates:

- `execution_state := ErrorRecovery`

12. `RetryRequested`

Preconditions:

- `execution_state = ErrorRecovery`
- `active_run = run_id`

State updates:

- `execution_state := CallingLlm`

13. `FatalFailure`

Preconditions:

- `execution_state ∈ {ApplyingPrimitive, CallingLlm, WaitingForOps, DrainingBoundary, ErrorRecovery}`
- `active_run = run_id`

State updates:

- `execution_state := Failed`
- `terminal_outcome := Failed`

Effect:

- `RunFailed(run_id)`

### Cancellation

14. `CancelRequested`

Preconditions:

- `execution_state ∈ {ApplyingPrimitive, CallingLlm, WaitingForOps, DrainingBoundary, ErrorRecovery}`
- `active_run = run_id`

State updates:

- `execution_state := Cancelling`

15. `CancellationObserved`

Preconditions:

- `execution_state = Cancelling`
- `active_run = run_id`

State updates:

- `execution_state := Cancelled`
- `terminal_outcome := Cancelled`

Effect:

- `RunCancelled(run_id)`

### Return to ready

16. `AcknowledgeTerminal`

Preconditions:

- `execution_state ∈ {Completed, Failed, Cancelled}`
- `active_run = run_id`

State updates:

- `execution_state := Ready`
- `active_run := None`
- `primitive_kind := None`
- `tool_calls_pending := 0`
- `boundary_count := 0`
- `terminal_outcome := None`

## Invariants

The machine must preserve:

1. `Ready` iff no run is active.
2. every non-`Ready` state has exactly one active run.
3. `WaitingForOps` implies `tool_calls_pending > 0`.
4. immediate primitives never enter `CallingLlm`, `WaitingForOps`, or
   `ErrorRecovery`.
5. terminal states have matching `terminal_outcome`.
6. a run cannot complete without at least one boundary application.
7. cancellation and failure are terminal for the active run until runtime
   acknowledges them.

## Implementation Anchors

Current `0.4` anchors include:

- `meerkat-core/src/state.rs`
- `meerkat-core/src/agent/state.rs`
- `meerkat-core/src/lifecycle/*.rs`
- `meerkat-runtime/src/runtime_loop.rs`

The biggest `0.5` work here is not inventing a new loop from nothing. It is
making runtime-driven execution universal and removing the legacy host-loop
coordination path from `Agent`.
