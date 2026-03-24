# Meerkat 0.5 Host-Mode Cutover Spec

Status: normative `0.5` cutover spec

## Purpose

This document defines how Meerkat deletes the legacy direct-agent host-mode
path and replaces it with one canonical runtime-owned execution path.

Current deletion target:

- `drain_comms_inbox()` as an ordinary execution owner
- `run_keep_alive*()` as a long-lived direct execution loop
- direct continuation injection from host loop
- direct batching/routing/injection in `comms_impl.rs` as the authoritative
  runtime behavior

## Current Problem

Today host-mode still has a second ordinary execution path:

- comms inbox
- classification/routing in `comms_impl.rs`
- direct session injection
- direct `Agent::run()` / `run_with_events()` / `run_pending_inner()`

That path duplicates behavior already present in the runtime path and is the
main source of late-bound ambiguity.

`0.5` deletes it.

## Final Owner Map

### `PeerCommsMachine`

Owns:

- transport/runtime inbox
- trust/auth snapshotting
- normalization of inbound peer/event traffic
- request/response registry
- interaction reservation/subscriber registry
- conversion into runtime admission candidates

### `RuntimeControlMachine` + `RuntimeIngressMachine`

Own:

- admission
- canonical ordering
- wake/drain scheduling
- boundary drains
- continuation scheduling
- control-plane preemption

### `TurnExecutionMachine`

Owns:

- one run at a time
- run-local state transitions
- emitted run effects/events

It does **not** own host idle waiting or inbox draining in the final design.

## Behaviors That Must Survive The Cutover

The legacy path does a number of things that the replacement must preserve
explicitly.

### 1. Actionable peer work triggers ordinary runtime admission

The following must become admitted runtime work exactly once:

- actionable peer messages
- actionable peer requests
- external `PlainEvent` style inputs

They must no longer call `Agent::run()` directly from the host loop.

### 2. Inline-only peer work remains inline-only

The following remain inline-only runtime/session effects:

- terminal peer responses
- silent requests
- peer lifecycle summaries

They do not directly become "start a new turn now" just because they arrived.

### 3. Terminal responses can still schedule continuation

If terminal peer responses are injected and no other admitted work in that
drain cycle starts a turn, runtime must still schedule a continuation run.

Final rule:

- host/comms does **not** synthesize continuation directly
- `RuntimeControlMachine` owns continuation scheduling as a runtime effect

`RuntimeInputSink::accept_continuation()` is transitional and should be deleted
by the end of `0.5`.

### 4. Interaction-scoped subscribers still resolve correctly

The current host loop preserves isolated terminal events for subscriber-bound
interactions.

That behavior must survive, but as a runtime/comms bridge contract rather than
as a direct `event_tap` trick in the host loop.

### 5. Request/response correlation survives

Current request/response behavior relies on interaction IDs, request IDs, and
subscriber correlation.

In the final design:

- `PeerCommsMachine` remains the owner of outbound request/response registry
- runtime receives normalized peer inputs with correlation metadata
- runtime does **not** become the owner of transport-side request bookkeeping

### 6. DISMISS/stop never becomes ordinary work ingress

Authority commands remain on the control plane.

`DISMISS` or equivalent control traffic should map to:

- `RunControlCommand::StopRuntimeExecutor`
- or an equivalent runtime control command

It should no longer survive as a stringly-typed side flag checked by the host
loop.

## Canonical Replacement Bridge

The replacement bridge is:

- `RuntimeCommsBridge`

It supersedes the current limited `RuntimeCommsInputSink`.

### Required behavior

For admitted peer work:

1. `PeerCommsMachine` normalizes transport input into runtime input:
   - `Input::Peer(...)`
   - `Input::ExternalEvent(...)`
2. it submits the input through
   `RuntimeSessionAdapter::accept_input_with_completion(...)`
3. if a completion handle is returned, the bridge retains it against the
   reserved interaction/subscriber scope
4. when the completion handle resolves, the bridge asks `PeerCommsMachine` to:
   - emit terminal subscriber events
   - mark the interaction reservation complete

This is the authoritative replacement for today's direct host-loop
subscriber/tap handling.

## Interaction Reservation Contract

`PeerCommsMachine` remains the owner of interaction reservations.

Required bridge data:

- `interaction_id`
- optional subscriber handle
- optional completion notification scope

Runtime does not need to understand transport internals.
It only needs an opaque reservation key that is handed back on completion.

Final completion effect shape:

- `ResolveInteractionReservation(interaction_id, terminal_outcome)`

Owned by:

- emitted by runtime bridge/runtime control path
- executed by `PeerCommsMachine`

## Response / Continuation Contract

The final continuation rule is:

1. terminal response admitted as normalized `Input::Peer` with
   `PeerConvention::ResponseTerminal`
2. response is routed as inline context injection
3. if that drain cycle produces no turn-triggering admitted work,
   `RuntimeControlMachine` emits `ScheduleContinuation`
4. runtime internally admits a system-generated continuation primitive/input
5. `TurnExecutionMachine` executes that continuation

The host-side bridge never synthesizes continuation directly in the final
system.

## Boundary Ownership

Final admissible boundaries are runtime-owned.

The named boundaries for cutover purposes are:

- `RuntimeIdleWait`
- `RunStart`
- `RunBoundary`
- `CooperativeYield`

Rules:

- comms no longer drains itself at these boundaries
- the runtime loop decides when ingress is drained
- `Agent::drain_comms_inbox()` no longer participates in ordinary execution

## Checkpointing Rule

Current host mode checkpoints after inline injections and after direct runs.

In the final design:

- runtime/session owners checkpoint based on admitted-input consumption and
  run-completion boundaries
- host/comms no longer owns checkpoint timing

## Replacement For Direct Host-Mode APIs

### `run_keep_alive()` / `run_keep_alive_with_events()`

Final meaning:

- compatibility façades that attach comms/runtime bridges and then delegate to
  runtime-owned idle/execution orchestration

They must not remain independent long-lived control loops.

### `drain_comms_inbox()`

Final status:

- compatibility/testing helper only, or deleted entirely

It must not remain part of ordinary execution.

### `host_drain_active`

Final status:

- deleted

Its job disappears once the host loop no longer owns the drain path.

## Required Runtime APIs

To complete the cutover, the runtime/session seam must provide:

- `accept_input_with_completion(...)` for comms-admitted work
- control-plane send for runtime stop/cancel commands
- runtime-owned continuation scheduling
- runtime-owned completion resolution hooks for reserved interactions

## Staged Landing Plan

### Step 1. Bridge completion-aware comms admission

Land the new runtime/comms bridge that:

- admits peer/event work through `accept_input_with_completion(...)`
- retains reservation/completion linkage
- resolves subscriber completions when runtime finishes the input

### Step 2. Move continuation ownership into runtime

Land runtime-owned continuation scheduling for terminal response injection and
delete host-triggered continuation admission.

### Step 3. Move authority commands fully onto control plane

Replace host-loop `dismiss_received()` ownership with explicit runtime control
delivery.

### Step 4. Narrow `run_keep_alive*()` into façades

Keep any user-facing compatibility API shape, but remove long-lived direct
execution ownership from `Agent`.

### Step 5. Delete the direct-agent path

Delete or demote as non-authoritative:

- `drain_comms_inbox()` as runtime owner
- direct classification/routing/injection in `comms_impl.rs`
- direct batching + `self.run()` from host loop
- host-synthesized continuation
- `host_drain_active`

## Completion Criteria

The cutover is complete only when all are true:

1. ordinary peer/event work reaches `TurnExecutionMachine` only through runtime
   admission
2. terminal response continuation is scheduled by runtime, not host code
3. interaction-scoped subscriber completion is resolved by runtime completion
   handles plus comms reservation effects
4. `DISMISS`/stop is control-plane owned
5. `Agent` no longer owns a second long-lived host idle/drain loop
