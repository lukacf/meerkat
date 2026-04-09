# Meerkat M1 Freeze

Status: frozen exact-current-state asset

This note closes `M1` from `meerkat-cutover-checklist.md`.

It captures the exact current live Meerkat semantics for interrupt and cancel
behavior at the runtime/session boundary, and it explicitly classifies what is
still target-state only.

## Scope

This freeze covers:

- `RuntimeSessionAdapter::interrupt_current_run`
- runtime-adapter delivery of `RunControlCommand::InterruptYielding`
- session-layer cooperative interrupt via `interrupt_yielding`

This freeze does **not** claim that every authority verb already has a live
lowering.

There is no longer a live or compatibility local wait-tool builder/binder seam
at this boundary. During the rebase audit, the briefly resurrected
wait-interrupt builder/binder API was removed again because it had no real
consumer on the current Meerkat path, so the frozen boundary is back to the
runtime/session seams only.

## Exact current semantics

### 1. `interrupt_current_run`

Exact current live behavior:

- without an attached runtime loop, `interrupt_current_run` rejects with
  `NotReady { state: Idle }`
- with an attached runtime loop, `interrupt_current_run` enqueues a
  `CancelCurrentRun` control command
- that control command does **not** interrupt an in-flight `apply()`
- the current run, current-run binding, and completion waiter remain live until
  `apply()` returns
- once `apply()` returns, the queued cancel drains through the executor control
  seam and the runtime returns to `Attached`

This is a deferred hard-cancel request, not an in-flight runtime-side state
mutation.

### 2. Runtime-adapter `InterruptYielding`

Exact current live behavior:

- a running peer message can emit `InterruptYielding`
- while the current `apply()` is still blocked, the peer input remains queued
- the `InterruptYielding` control command does **not** hard-cancel the current
  run
- it drains after the current `apply()` returns and before the next queued
  input starts

This is a deferred cooperative interrupt request at the runtime-adapter layer.

### 3. Session-layer cooperative interrupt

Exact current live behavior:

- `interrupt_yielding` reaches the cooperative wait/yield channel
- a cooperative wait can wake promptly and return normally with an interrupted
  result
- this does **not** call the agent hard-cancel path

This is distinct from `CancelCurrentRun`.

## Exact-vs-target-state classification

### `cancel_after_boundary`

`cancel_after_boundary` remains present in:

- `TurnExecutionAuthority`
- generated machine/schema assets
- snapshot surfaces that expose authority state

But the repo trace for `TurnExecutionInput::CancelAfterBoundary` found only
authority-local uses and tests. No concrete live lowering was found in:

- `meerkat-runtime`
- `meerkat-session`
- `agent runner`
- adapter/runtime loop control paths

So the exact current-state classification is:

- `cancel_after_boundary` is **not** a discovered live Meerkat runtime/session
  boundary action today
- it is currently an authority-local or target-state verb
- it must not be treated as a frozen live cutover action until a real lowering
  exists and is captured by the Meerkat lowering map

That means `M1` is complete for the exact current live boundary, and
`cancel_after_boundary` moves out of “interrupt ambiguity” and into
“future lowering or explicit post-freeze target-state landing”.

## Proof inventory

Runtime-layer proofs:

- `interrupt_current_run_returns_not_ready_without_attached_loop`
- `interrupt_current_run_on_attached_runtime_is_deferred_until_apply_finishes`
- `running_peer_message_interrupt_yielding_drains_before_next_apply`

Session-layer proof:

- `test_interrupt_yielding_interrupts_cooperative_wait_without_cancel`

## Reviewer Verification Lane

The narrow verification lane for this freeze asset is:

- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-session --test ephemeral_contract test_interrupt_yielding_interrupts_cooperative_wait_without_cancel`
- `rg -n "CancelAfterBoundary|cancel_after_boundary|interrupt_current_run|InterruptYielding|interrupt_yielding|CancelCurrentRun" meerkat-runtime meerkat-session meerkat-core meerkat/src docs/architecture/two-kernel-research`

What a passing review means:

- the plain and attached `interrupt_current_run` proofs still hold
- the runtime-adapter `InterruptYielding` proof still holds
- the session-layer cooperative interrupt proof still holds
- the repo trace still does not reveal a newly discovered live lowering for
  `cancel_after_boundary`

## Reopen Conditions

`M1` should be reopened only if one of the following becomes true:

- a real runtime/session lowering for `cancel_after_boundary` is added
- `interrupt_current_run` stops being deferred on attached runtimes
- `InterruptYielding` starts hard-canceling the current run
- session-layer `interrupt_yielding` begins calling the hard-cancel path

The following do **not** reopen `M1` by themselves:

- new exact-current lifecycle proofs around steer, detach, destroy, or retire
- detached-wake changes that do not alter the interrupt/cancel boundary
- additional documentation or lowering-map work that does not change live
  behavior

## Freeze Posture

This is now a review-ready exact-current-state asset.

The rebase audit for this asset is closed: widened workspace lib, test,
test-check, and all-target compile lanes are green, so merge fallout is no
longer an active blocker to reviewing or freezing this boundary.

It is not target-state architecture. It is the explicit current cutover
classification for the interrupt/cancel boundary, including the decision to
exclude `cancel_after_boundary` until a real lowering exists.

## Freeze decision

`M1` is considered frozen for the exact current Meerkat cutover boundary.

What is frozen:

- hard cancel request delivery semantics
- cooperative interrupt delivery semantics
- the distinction between the two

What is explicitly not frozen as live behavior:

- `cancel_after_boundary` lowering

That remaining gap is no longer an `M1` blocker. It is a lowering-scope issue
for later Meerkat cutover work.
