# Meerkat Interrupt Freeze

Status: frozen exact-current-state asset

This note closes `K1` from `meerkat-cutover-checklist.md`.

It captures the exact current live Meerkat semantics for interrupt and cancel
behavior at the runtime/session boundary, and it explicitly classifies what is
still target-state only.

## Scope

This freeze covers:

- `MeerkatMachine::interrupt_current_run`
- runtime-adapter delivery of `RunControlCommand::InterruptYielding`

This freeze does **not** claim that every authority verb already has a live
lowering.

There is no longer a live or compatibility local wait-tool builder/binder seam
at this boundary. During the rebase audit, the briefly resurrected
wait-interrupt builder/binder API was removed again because it had no real
consumer on the current Meerkat path, so the frozen boundary is back to the
runtime/session-adapter seams only.

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

### 3. No distinct session-layer interrupt seam is currently discovered

Exact current live behavior:

- the current codebase has no distinct `meerkat-session` interrupt verb,
  control command, or contract test named `interrupt_yielding`
- the live top-level seam is the runtime-adapter delivery of
  `RunControlCommand::InterruptYielding` into the attached executor control
  channel
- any deeper cooperative-yield behavior is therefore below the exact-current
  top-level Meerkat boundary, not a second frozen session-layer seam

That means `K1` freezes the runtime-adapter interrupt seam exactly as it exists
today, without pretending there is a second current session-layer action.

## Exact-vs-target-state classification

### `cancel_after_boundary`

`cancel_after_boundary` is now part of the exact current live boundary. The
current lowering runs through:

- `MeerkatMachine::cancel_after_boundary(...)`
- `SessionService::cancel_after_boundary(...)`
- the session-owned boundary-cancel flag observed by the agent runner at safe
  phases

So the exact current-state classification is:

- `cancel_after_boundary` is a discovered live Meerkat runtime/session boundary
  action
- it is deferred boundary cancel, not a hard interrupt
- it is part of the frozen live cutover surface for `K1`

## Proof inventory

Runtime-layer proofs:

- `interrupt_current_run_returns_not_ready_without_attached_loop`
- `interrupt_current_run_on_attached_runtime_is_deferred_until_apply_finishes`
- `cancel_after_boundary_returns_not_ready_without_attached_loop`
- `cancel_after_boundary_on_attached_runtime_is_deferred_until_apply_finishes`
- `running_peer_message_interrupt_yielding_drains_before_next_apply`

## Reviewer Verification Lane

The narrow verification lane for this freeze asset is:

- `cargo test -p meerkat-runtime --lib interrupt_current_run_returns_not_ready_without_attached_loop`
- `cargo test -p meerkat-runtime --lib interrupt_current_run_on_attached_runtime_is_deferred_until_apply_finishes`
- `cargo test -p meerkat-runtime --lib cancel_after_boundary_returns_not_ready_without_attached_loop`
- `cargo test -p meerkat-runtime --lib cancel_after_boundary_on_attached_runtime_is_deferred_until_apply_finishes`
- `rg -n "CancelAfterBoundary|cancel_after_boundary|interrupt_current_run|InterruptYielding|CancelCurrentRun" meerkat-runtime meerkat-session meerkat-core meerkat/src docs/architecture/two-kernel-research`

What a passing review means:

- the plain and attached `interrupt_current_run` proofs still hold
- the plain and attached `cancel_after_boundary` proofs still hold
- the runtime-adapter `InterruptYielding` proof still holds
- no distinct session-layer interrupt seam has been reintroduced
- the repo trace still shows the canonical live boundary-cancel lowering
  through the runtime/session seam

## Reopen Conditions

`K1` should be reopened only if one of the following becomes true:

- `interrupt_current_run` stops being deferred on attached runtimes
- `InterruptYielding` starts hard-canceling the current run
- a distinct top-level session-layer interrupt action is introduced
- the live `cancel_after_boundary` lowering changes its authority seam or safe
  observation phases

The following do **not** reopen `K1` by themselves:

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
classification for the interrupt/cancel boundary, including the now-landed
live `cancel_after_boundary` lowering.

## Freeze decision

`K1` is considered frozen for the exact current Meerkat cutover boundary.

What is frozen:

- hard cancel request delivery semantics
- cooperative interrupt delivery semantics at the runtime-adapter seam
- boundary-cancel request delivery semantics at the runtime/session seam
- the distinction between the two
- the explicit absence of a second current session-layer interrupt lowering
