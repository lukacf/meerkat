# meerkat-runtime cascade — triage (#32 Class W6)

**Baseline**: integration tip `1bd8634a6` (post-W1-facade + W7). One drive-by fix for the ops_lifecycle `"peer-id"` literals landed as part of this triage; see "Drive-by" below.

**Command**: `perl -e 'alarm 240; exec @ARGV' ./scripts/repo-cargo nextest run -p meerkat-runtime --lib --no-fail-fast`

**Result pre-drive-by**: `460 tests run: 420 passed, 40 failed, 3 skipped`.
**Result post-drive-by** (2 ops_lifecycle peer-id sites fixed in same commit as this doc): 460 tests, **422 passed, 38 failed**, 3 skipped.

---

## Bucket counts

Panic-message clustering on the 40 failures:

| Count | Class tag | Bucket |
|-------|-----------|--------|
| 20 | W6.1 | `assertion left == right failed` on phase transitions — spine-snapshot reader sees stale `Running`/`Idle` where expected `Retired`/`Destroyed` |
| 5 | W6.2 | `Elapsed(())` phase-transition timeouts — same family as W6.1 but using the runtime-loop variant (test waits for phase change that never arrives) |
| 4 | W6.3 | composition-producer route-schema missing `session_id` field — same root as `#31 Class A1` (seam), different route |
| 4 | W6.4 | `peer_ingress_owner` returns `Unattached` where test expects `MobOwned`/`SessionOwned` — owner-state persistence gap |
| 3 | W6.5 | `missing projected field session_id` / `routed input did not project session_id ... — no session can be resolved` — consumer-side projection |
| 2 | W6.6 | `assertion failed: !snapshot.drain.slot_present` or similar comms-drain state-projection checks |
| 2 | **D1 (drive-by)** | `ops_lifecycle` tests hardcoded `"peer-id"` strings rejected by post-#24 `PeerId::parse` — same class as #32 W5.3. **Fixed in this commit.** |

**Class groupings by root-cause, not panic-message**:

- **A (composition seam)** = W6.3 + W6.5 (7 tests) — `binding_request_reaches_meerkat` route / `RequestRuntimeBinding` variant / producer-consumer session_id mismatch; extension of the #31 A1 fix (`9e3b31ec4`) to a different route in the same seam schema.
- **B (spine-snapshot phase-transition regression)** = W6.1 + W6.2 + W6.6 (27 tests) — snapshot reader reports stale control-phase + drain-slot state after `retire`/`destroy`/`stop_runtime_executor`.
- **C (peer-ingress owner persistence gap)** = W6.4 (4 tests) — `maybe_spawn_mob_comms_drain` or equivalent owner-establishment path doesn't persist to the state read by `peer_ingress_owner()`.
- **D (ops_lifecycle peer-id)** = 2 tests — drive-by, fixed here.

Additional 1 singleton: `refuses_unwired_consumer_typed` expects `DispatchRefusal::UnwiredConsumer` but gets something else. Likely a ripple from Class A once the route schema changes.

---

## Class A — Composition `binding_request_reaches_meerkat` route missing `session_id` field (7 tests)

### Affected tests

- `composition::route_table::tests::resolves_request_runtime_binding_to_prepare_bindings`
- `composition::tests::dispatches_mob_routed_effect_to_meerkat_consumer`
- `composition::tests::refuses_unwired_consumer_typed` (likely-ripple)
- `meerkat_machine::composition::tests::pinned_surface_rejects_mismatched_runtime_id`
- `meerkat_machine::composition::tests::prepare_bindings_requires_all_three_fields`
- `meerkat_machine::composition::tests::unknown_variant_is_refused_typed`
- `meerkat_machine::composition::tests::unpinned_surface_requires_projected_runtime_id_for_retire`

### Panics

**route_table test**:
```
assertion `left == right` failed
  left:  [("agent_runtime_id", "agent_runtime_id"), ("fence_token", "fence_token"),
          ("generation", "generation"), ("session_id", "session_id")]
  right: [("agent_runtime_id", "agent_runtime_id"), ("fence_token", "fence_token"),
          ("generation", "generation")]
```

Test expects 4 field-mappings including `session_id`; schema declares only 3.

**composition dispatch test**:
```
well-formed routed effect:
  MissingProducerField { route: RouteId("binding_request_reaches_meerkat"),
                         variant: EffectVariantId("RequestRuntimeBinding"),
                         field: FieldId("session_id") }
```

Producer-side route declaration for `binding_request_reaches_meerkat` / `RequestRuntimeBinding` lacks the `session_id` projection.

**meerkat_machine consumer tests**:
```
routed input did not project `session_id` and surface is not pinned to a session
  — no session can be resolved
```

Consumer-side rejection at `MeerkatConsumerSurface::resolve_session` — same code path that `#31 A1` fix rewired. Those fixes targeted the `PrepareBindings` / `Retire` / `Destroy` routes; the `RequestRuntimeBinding` route here wasn't updated.

### Root cause

The `meerkat_mob_seam` composition schema in `meerkat-machine-schema/src/catalog/` (or `meerkat-runtime/src/generated/meerkat_mob_seam.rs` generated file) declares three routes. `#31 A1` extended `PrepareBindings` / `Retire` / `Destroy` to carry a typed `session_id` field. The fourth route — `binding_request_reaches_meerkat` carrying variant `RequestRuntimeBinding` — was missed.

### `git log -S` root

```
git log --oneline -S "binding_request_reaches_meerkat" --all
```

Not executed in this pass. Implementer should grep for the `RouteBinding` / `RouteId` construction site for `binding_request_reaches_meerkat` in the schema crate and extend its `consumer_projections` / similar field-map with `session_id`. The mirror on the producer side (`RequestRuntimeBinding` effect variant) needs a `session_id` field.

### Fix shape

Extend the seam-schema declaration of `binding_request_reaches_meerkat`:
- Producer side: `RequestRuntimeBinding` effect variant gains a `session_id: SessionId` field.
- Consumer side: `PrepareBindings` input variant (if the consumer-side target — check the route's target) gets `session_id` added to its projection map.
- The `EffectExtractor` for the producer must populate the new field.

Most of the plumbing mirrors what `9e3b31ec4` did for the other three routes. The fix likely fits in one commit touching the schema file + the 1-2 producer code sites that build `RequestRuntimeBinding`.

### Scope estimate

**Small-to-medium, 1 commit**. Mechanical if the `9e3b31ec4` pattern is followed.

### Commit message preamble

```
fix(seam): extend binding_request_reaches_meerkat route with session_id (#32 W6 Class A)

Commit `9e3b31ec4` wired `session_id` into the mob→meerkat seam's
`PrepareBindings` / `Retire` / `Destroy` routes (#31 Class A1). The
seam schema's fourth route — `binding_request_reaches_meerkat`
carrying the `RequestRuntimeBinding` variant — was missed.

Seven `meerkat-runtime` composition tests fail at the producer-side
declaration check (`MissingProducerField { ..., field: "session_id" }`)
and consumer-side `resolve_session` rejection (`routed input did not
project session_id`).

Extend the route schema with the `session_id` field on both
producer and consumer sides; mirror the `EffectExtractor` update
that `9e3b31ec4` applied to the other three routes.
```

### Tests unblocked

7 tests + likely 1 ripple (`refuses_unwired_consumer_typed`).

---

## Class B — Spine-snapshot phase-transition regression (27 tests)

### Affected tests (representative sample)

- `meerkat_machine::tests::meerkat_machine_spine_snapshot_destroy_splits_completion_and_wait_all_lifetimes`
- `meerkat_machine::tests::meerkat_machine_spine_snapshot_preserves_wait_all_after_retire`
- `meerkat_machine::tests::meerkat_machine_spine_snapshot_preserves_wait_all_after_retire_with_runtime_loop`
- `meerkat_machine::tests::meerkat_machine_spine_snapshot_retire_clears_steered_waiter_and_steer_queue`
- `meerkat_machine::tests::meerkat_machine_spine_snapshot_attached_steered_prompt_destroy_splits_completion_and_wait_all_lifetimes`
- `meerkat_machine::tests::meerkat_stop_runtime_executor_clears_silent_intent_overrides`
- ...~18 more in the same `meerkat_machine_spine_snapshot_*` and `meerkat_stop_runtime_executor_*` families.

### Panics

Two variants:

**Variant B.1** — sync tests (non-runtime-loop):
```
assertion `left == right` failed
  left:  Idle   (or Running)
  right: Retired (or Destroyed)
```
Tests call `retire(runtime_id).await` then `meerkat_machine_spine_snapshot(&session_id).await` then assert `snapshot.control.phase == Retired`. The snapshot reader returns `Idle`.

**Variant B.2** — runtime-loop tests:
```
runtime should return to Retired once the preserved work finishes draining: Elapsed(())
runtime phase transition should complete: Elapsed(())
phase transition: Elapsed(())
attached stop should eventually publish the Stopped phase: Elapsed(())
```
Tests use `tokio::time::timeout(...)` waiting for phase to transition; timeout fires.

Both variants are the same root cause: the `retire` / `destroy` / `stop_runtime_executor` control-plane commands aren't transitioning the snapshot reader's `phase` field. Either:
- (B-i) The command's DSL input fires but the snapshot reader consults a different authority that didn't update.
- (B-ii) The command emits a DSL input that the machine rejects (no transition defined for the current phase), silently.
- (B-iii) There's an eventual-consistency gap where the snapshot reader has stale data after the command returns.

### Representative example

`meerkat_machine_spine_snapshot_preserves_wait_all_after_retire` at `meerkat_machine_tests.rs:8942`:
```rust
let report = RuntimeControlPlane::retire(&*adapter, &runtime_id)
    .await
    .expect("retire should preserve the active wait_all carrier");
assert_eq!(report.inputs_abandoned, 0);
assert_eq!(report.inputs_pending_drain, 0);

let after_retire = adapter.meerkat_machine_spine_snapshot(&session_id).await.expect("...");
assert_eq!(after_retire.control.phase, RuntimeState::Retired);  // ← panics: got Idle
```

Diagnostic path: the `retire(...)` call returns a `report` with `inputs_abandoned: 0`, suggesting the retire code path ran. But the follow-up snapshot sees `phase == Idle`. If the pre-retire phase was already `Idle` (not `Running`), the `retire` command may be a no-op — the test may assume the runtime is `Running` before retire, but some init-sequence change causes it to be `Idle` at test start. The test-setup then never transitions the phase.

### Root cause hypothesis

Most likely: a wave-d-baseline restore-commit (`d5bddfe8d` or similar) changed the initial-phase invariant of `MeerkatMachine::ephemeral()` or changed which DSL-input the `retire` command emits, so the tests' pre-retire phase assumption is stale.

Alternative: the snapshot reader at `meerkat_machine_spine_snapshot()` reads from shell-shadow state that wave-a deleted without a replacement projection.

### `git log -S` root

Candidates to trace:
- `git log --oneline -S "meerkat_machine_spine_snapshot" --all`
- `git log --oneline -S "phase: RuntimeState::" --all`
- Recent changes to `meerkat-runtime/src/meerkat_machine/control_plane.rs` or wherever `retire`/`destroy`/`stop_runtime_executor` emit DSL inputs.

Not traced in this pass. 27 tests is a large enough cluster that dedicated investigation is warranted.

### Fix shape

**Two paths**:
- **(B.α)** If the snapshot reader is reading stale data, migrate it to the canonical DSL authority. Identify the snapshot's `control.phase` source; point it at the DSL `RuntimeState` field.
- **(B.β)** If the command's DSL input isn't firing the phase transition, restore the missing transition in the machine's DSL definition.

### Scope estimate

**Medium-to-large**. 27 tests is the biggest single cluster in meerkat-runtime. Needs dedicated investigation — probably a sub-task of its own.

### Commit message preamble

```
fix(meerkat-runtime): restore spine-snapshot phase projection after retire/destroy (#32 W6 Class B)

27 `meerkat_machine_spine_snapshot_*` / `meerkat_stop_runtime_executor_*`
tests fail because the `meerkat_machine_spine_snapshot(session_id).control.phase`
reader returns `Idle` (or `Running`) after control-plane `retire` /
`destroy` / `stop_runtime_executor` commands should have transitioned
the runtime-state phase to `Retired` / `Destroyed` / `Stopped`. The
commands' DSL inputs either don't fire or the snapshot reader consults
a stale shell-shadow instead of the DSL authority.

<TBD root trace>: either migrate the snapshot reader to the canonical
DSL `RuntimeState` field, or restore the missing DSL transition the
command's input was expected to fire.
```

### Tests unblocked

27.

---

## Class C — Peer-ingress owner persistence gap (4 tests)

### Affected tests

- `meerkat_machine::tests::attach_session_ingress_transitions_owner`
- `meerkat_machine::tests::attach_mob_ingress_transitions_owner`
- `meerkat_machine::tests::attach_mob_ingress_promotes_from_session_owned`
- `meerkat_machine::tests::mob_owned_drain_rejects_silent_session_downgrade`

### Panics

```
expected MobOwned, got Unattached
expected SessionOwned, got Unattached
expected MobOwned after promotion, got Unattached
expected MobOwned to survive downgrade attempt, got Unattached
```

### Representative test flow

```rust
let adapter = Arc::new(MeerkatMachine::ephemeral());
let session_id = SessionId::new();
adapter.register_session(session_id.clone()).await;
let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());
let mob_id = mob_dsl::MobId::from("mob-w2g-test");
adapter.maybe_spawn_mob_comms_drain(&session_id, Arc::clone(&comms_runtime), mob_id.clone()).await;

let owner = adapter.peer_ingress_owner(&session_id).await;
match owner {
    PeerIngressOwner::MobOwned { .. } => { /* expected */ }
    other => panic!("expected MobOwned, got {other:?}"),  // ← actually Unattached
}
```

`maybe_spawn_mob_comms_drain` is called; `peer_ingress_owner` returns `Unattached` immediately after. The ownership-establishment side-effect of `maybe_spawn_mob_comms_drain` isn't persisting to the state `peer_ingress_owner` reads.

### Root cause

Either:
- `maybe_spawn_mob_comms_drain` is short-circuiting (the `maybe_` suggests idempotent/conditional behavior — it may be checking a precondition that's false at test-time).
- The ownership write path goes through a DSL authority that the wave-d migration disconnected from the reader.

### `git log -S` root

```
git log --oneline -S "maybe_spawn_mob_comms_drain" --all
git log --oneline -S "PeerIngressOwner::" --all
git log --oneline -S "peer_ingress_owner" --all
```

### Fix shape

Trace from `maybe_spawn_mob_comms_drain` through to the ownership write. Either restore the missing write or fix the short-circuit condition. Likely small if the problem is a single-line guard; medium if the whole write-authority plumbing got unwired.

### Scope estimate

**Small-to-medium, 1 commit**.

### Commit message preamble

```
fix(meerkat-runtime): restore peer-ingress owner persistence after maybe_spawn_mob_comms_drain (#32 W6 Class C)

4 tests in `meerkat_machine::tests::attach_*` and `mob_owned_drain_rejects_silent_session_downgrade`
fail: after `maybe_spawn_mob_comms_drain(session_id, comms, mob_id)`
completes, `peer_ingress_owner(session_id)` returns `Unattached`
instead of `MobOwned { comms_runtime_id, mob_id }`.

<TBD root trace>: the ownership-establishment side effect of
`maybe_spawn_mob_comms_drain` isn't persisting through to the state
`peer_ingress_owner` reads. Either restore the ownership write, or
fix the short-circuit condition in the `maybe_` guard.
```

### Tests unblocked

4 (plus possibly 1-2 ripple).

---

## Drive-by fix (D1) — ops_lifecycle `peer-id` literals

### Affected tests (closed here)

- `ops_lifecycle::tests::peer_ready_requires_peer_expectation`
- `ops_lifecycle::tests::snapshot_includes_peer_handle`

### Panics

```
called `Result::unwrap()` on an `Err` value:
  "invalid peer_id: invalid peer id \"peer-id\":
   invalid character: expected an optional prefix of `urn:uuid:`
   followed by [0-9a-fA-F-], found `p` at 1"
```

Same class as #32 Class W5.3: hardcoded `"peer-id"` literals in `TrustedPeerDescriptor::test_only_unsigned(...)` calls rejected by post-#24 typed `PeerId::parse`. Two sites: `meerkat-runtime/src/ops_lifecycle.rs:1863, 2279`.

### Fix applied

Migrated both sites from `test_only_unsigned(name, "peer-id", addr)` to `test_only_unsigned_typed(name, PeerId::new(), addr)` using the typed helper added in `b67f52493`. Added `PeerId` import to the test module.

### Catching assertion

Before: 2 tests fail with the `peer-id` parse error.
After (this commit): 2 tests pass.

---

## Summary table

| Class | Count | Root | Scope | Independently fixable? |
|-------|-------|------|-------|------------------------|
| A (seam schema) | 7 | `binding_request_reaches_meerkat` route missing `session_id` field, extension of #31 A1 | Small-to-medium, 1 commit | Yes |
| B (spine snapshot) | 27 | Snapshot reader returns stale control-phase after `retire`/`destroy`; needs own root-trace | Medium-to-large, 1+ commits | Yes |
| C (peer-ingress owner) | 4 | `maybe_spawn_mob_comms_drain` doesn't persist ownership read by `peer_ingress_owner` | Small-to-medium, 1 commit | Yes |
| D (drive-by) | 2 | `peer-id` literals rejected by typed `PeerId::parse` | Tiny, already committed | ✅ closed |

### Priority for meerkat-runtime unblock

1. **A (7 tests)** — mechanical extension of a pattern already merged (#31 A1). Low risk, clear diff.
2. **C (4 tests)** — single production site, likely 1-line fix once traced.
3. **B (27 tests)** — biggest cluster, worth its own triage task (candidate #34-style or dedicated "spine-snapshot behavior" agent). Complex enough that jumping into a fix without root trace would be risky.

### Next concrete assignments

- If parallel resources available: one agent on A (mechanical), one on C (focused), one on B (own triage).
- If serial: A → C → B in priority order.

---

## Method notes

- Triage used `perl -e 'alarm N' ./scripts/repo-cargo nextest run -p meerkat-runtime --lib --no-fail-fast` with `&>` redirect to `/tmp/runtime-w6.log` to capture full output (macOS single-`>` truncates at buffer threshold).
- Panic-message bucketing via `awk '/panicked at/{getline line; print line}' | sort | uniq -c | sort -rn`.
- Representative-test deep-dive via `--nocapture -E 'test(name1) or test(name2) ...'` to surface `assertion left == right` bodies the summary line hides.
- `refuses_unwired_consumer_typed` singleton panic (`assertion failed: matches!(err, DispatchRefusal::UnwiredConsumer { .. })`) was assumed a Class A ripple — the test expects an unwired-consumer refusal but gets the `MissingProducerField` refusal (which happens first in the dispatcher's validation order). Fixing A should un-block it.
- Not all 40 test bodies were inspected individually — the `uniq -c` bucketing pattern covers ~38 of 40; the 2 ops_lifecycle tests were the drive-by.
