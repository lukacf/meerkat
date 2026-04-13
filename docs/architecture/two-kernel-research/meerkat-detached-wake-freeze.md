# Meerkat Detached-Wake Freeze

Status: frozen exact-current-state asset

This note closes `K2` from `meerkat-cutover-checklist.md`.

It captures the exact current live Meerkat semantics for detached wake and
continuation injection at the runtime loop boundary, and it explicitly
classifies the feed-backed path versus the legacy compatibility path.

## Scope

This freeze covers:

- background-operation completion visibility through the runtime completion feed
- runtime-loop detached wake injection of
  `ContinuationInput::detached_background_op_completed()`
- the legacy `DetachedWakeState` fallback when no completion feed is present

This freeze does **not** claim that detached wake is already fully integrated
with all turn/barrier semantics. It freezes the exact current wake and
continuation behavior only.

## Exact current semantics

### 1. Feed-backed detached wake is the canonical live path

For registered runtimes, `MeerkatMachine::register_session_with_executor`
wires both:

- a completion feed from `RuntimeOpsLifecycleRegistry`
- a `DetachedWakeState`

But the runtime loop always prefers the completion feed when it is present.

Exact current live behavior:

- new `BackgroundToolOp` completions become visible through the feed
- when the session is quiescent, the runtime loop injects
  `ContinuationInput::detached_background_op_completed()` through the canonical
  ingress seam
- this can happen from either:
  - the idle feed wait arm
  - the post-drain `maybe_inject_feed_wake(...)` helper
- fresh runtime-backed cursor state now seeds from the actual cursor values,
  even when they are zero, so a newly spawned loop cannot silently skip a
  background completion that lands before its first select iteration

### 2. Detached wake defers while the session is not quiescent

Exact current live behavior:

- if a new background completion arrives while the session is running or still
  has active input truth, detached wake does **not** inject immediately
- on the feed-backed path, `observed_seq` is intentionally not advanced in the
  non-quiescent branch
- that keeps the completion visible for the next wake cycle so it can be
  injected later at a quiescent point

### 3. Only `BackgroundToolOp` completion triggers detached wake

Exact current live behavior:

- `BackgroundToolOp` terminal completion is the only completion kind that
  triggers detached wake
- `MobMemberChild` completions do **not** trigger detached wake continuation
  injection
- those member-child paths already surface through other runtime/comms seams
  and must not synthesize a second continuation

### 4. The injected continuation shape is stable

Exact current live behavior:

- `ContinuationInput::detached_background_op_completed()` is:
  - `Derived`
  - invisible to transcript/operator surfaces
  - `HandlingMode::Steer`
  - `RuntimeExecutionKind::ResumePending`

### 5. Legacy `DetachedWakeState` is compatibility behavior

Exact current live behavior:

- when no completion feed is present, `DetachedWakeState` remains as a
  compatibility fallback
- producer side:
  - terminal `BackgroundToolOp` sets `pending = true`
  - it does **not** set the legacy `signaled` latch
- consumer side:
  - the post-drain legacy helper does **not** inject inline
  - it sets `signaled = true` and notifies the idle arm
  - the idle arm later injects the continuation and
    `clear_legacy_detached_wake_signal(...)` clears both `pending` and
    `signaled`

This path is frozen as compatibility behavior, not as the canonical registered
runtime path.

## Exact-vs-compatibility classification

### Exact current canonical path

The exact current canonical detached-wake path for registered runtimes is:

- completion feed
- runtime-loop quiescence check
- continuation injection through canonical ingress

### Legacy compatibility path

The exact current compatibility fallback is:

- `DetachedWakeState`
- legacy `signaled` latch
- notify-driven idle-arm injection

This path remains part of the runtime loop, but it is compatibility behavior
that only applies when no completion feed is available.

## Proof inventory

Feed-backed direct proofs:

- `maybe_inject_feed_wake_feed_path_injects_inline_continuation_when_quiescent`
- `choke_004_feed_backed_idle_runtime_injects_continuation_without_manual_trigger`
- `choke_004_idle_runtime_wakes_on_detached_op_completion`
- `choke_004_five_completions_produce_one_coalesced_wake`
- `choke_004_completion_during_running_defers_wake`
- `choke_004_mob_member_child_completion_does_not_trigger_idle_wake`

Legacy direct proofs:

- `background_terminal_sets_detached_wake_pending_without_signaled_latch`
- `maybe_inject_feed_wake_legacy_path_sets_signaled_without_inline_injection`
- `clear_legacy_detached_wake_signal_resets_pending_and_signaled`

Continuation-shape proof:

- `unit_003_continuation_helper_builds_derived_invisible_steer`

## Freeze decision

`K2` is considered frozen for the exact current Meerkat cutover boundary.

What is frozen:

- canonical feed-backed detached-wake behavior
- deferred non-quiescent wake behavior
- completion-kind filtering
- continuation shape
- legacy compatibility fallback behavior

That means `K2` no longer blocks Meerkat freeze. The next blocker is `K3`:
turn / ops / barrier coupling.
