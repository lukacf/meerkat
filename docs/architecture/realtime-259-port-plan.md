# Realtime-voice port onto codex/machine-dsl-completion (PR #259)

Plan produced 2026-04-17 as the rebase strategy after Luka pointed out that
"compiles" ≠ "implemented architecturally correct." The pre-259 realtime-voice
branch added a runtime-owned state machine plus several orchestration methods
that are semantic, not just mechanical. This document names each addition and
decides DSL-extend vs. shell-mechanic per the meerkat-architecture skill
(post-DSL cutover).

## Inventory of realtime-voice runtime additions that touch machines

Source: `diff cfe85b55a..backup/realtime-voice-pre-259-rebase --
meerkat-runtime/src/meerkat_machine.rs`.

### A. Semantic state (MUST become DSL state)

**`RuntimeRealtimeAttachmentAuthority`** — per-session binding-state machine.

Fields:
- `intent_present: bool` — durable projection: does the operator intend live voice on this session.
- `binding_state: RuntimeRealtimeBindingState` — `Unbound` | `BindingNotReady(epoch)` | `BindingReady(epoch)` | `ReplacementPending(epoch)`.
- `reattach_required: bool` — last-known-binding was force-revoked; consumer must re-call `attach_live`.
- `next_authority_epoch: u64` — monotonic counter that stamps every new binding authority.

Guarded transitions (implemented as `impl` methods in the pre-rebase branch, but semantically a machine):

| Operation | Guard | Update |
|---|---|---|
| `project_intent(present)` | (none) | `intent_present = present` |
| `begin_binding(session_id)` | (none) | `binding_state = BindingNotReady(next_epoch); reattach_required = false; next_epoch += 1` → returns `SignalAuthority { session_id, authority_epoch }` |
| `replace_binding(session_id)` | (none) | `binding_state = ReplacementPending(next_epoch); reattach_required = false; next_epoch += 1` → returns `SignalAuthority` |
| `detach_binding` | (none) | `binding_state = Unbound; reattach_required = false; next_epoch += 1` |
| `require_reattach` | (none) | `reattach_required = true; binding_state = Unbound; next_epoch += 1` |
| `publish_signal(authority, status)` | `authority.epoch == active_epoch` AND `status ∈ {BindingNotReady, BindingReady, ReplacementPending}` | `binding_state` advances per status (preserving current epoch); `reattach_required = false`. Terminal/projection statuses (Unattached, IntentPresentUnbound, ReattachRequired) are rejected. |
| `validate_authority(authority)` | `active_epoch.is_some() AND authority.epoch == active_epoch` | read-only |

Status projection: `status() → RealtimeAttachmentStatus` derived from `{binding_state, intent_present, reattach_required}`.

**Dogma verdict:** one semantic fact, one owner. Machines own semantics. This
is not a projection — it's a state machine with explicit guards and authority
epochs. It must live in MeerkatMachine DSL. My current implementation
(`meerkat-runtime/src/meerkat_machine/realtime_attachment.rs` shell module)
violates the dogma.

### B. Commands already in `meerkat_machine_types.rs` (from the squash)

- `MeerkatMachineCommand::RuntimeRealtimeAttachmentStatus { session_id }`
- `MeerkatMachineCommand::IngestRealtimeAttachmentSignal { session_id, status }`
- `MeerkatMachineCommandResult::RealtimeAttachmentStatus(RealtimeAttachmentStatus)`
- `RealtimeAttachmentStatus` enum (projection type)
- `RealtimeAttachmentSignalAuthority` struct (token type)

**Verdict:** command envelopes and projection types stay. They route into DSL
inputs once DSL has the state.

### C. Orchestration methods (legitimately shell composition)

**`reconfigure_live_topology(authority, request) → Result<new_authority, _>`** —
live LLM-identity swap under an active realtime binding. Composes:
1. `prepare_reconfigure_session_llm_command` (DSL-prepared)
2. `revoke_realtime_attachment_ingress` (rotates authority in DSL, returns pre-commit snapshot)
3. `drive_live_topology_boundary` (awaits run boundary)
4. `detach_live` (DSL input)
5. `host.apply_live_session_llm_identity` (meerkat-session host side effect)
6. `host.apply_live_session_tool_visibility_state` (host side effect)
7. Error recovery: `restore_realtime_attachment_authority` (puts pre-commit snapshot back into DSL) or `fail_live_topology_after_detach` (DSL RequireRealtimeReattach)

**Verdict:** shell composition of DSL primitives + host-owned side effects.
Stays in runtime shell; each mutation routes through DSL.

**`maybe_spawn_comms_drain(session_id, keep_alive, comms_runtime)`** — spawns or
aborts the per-session comms drain task. Already dispatches through
`MeerkatMachineCommand::SetPeerIngressContext` (DSL command). Mechanics of
task spawning live in shell.

**Verdict:** mechanics, stays in shell. Needs porting as-is.

### D. Other realtime-touched areas

**`input_ledger.rs` — `InputState::apply(InputLifecycleInput::...)`**
Pre-DSL pattern. InputLifecycle authority was absorbed into MeerkatMachine DSL.
Shell callers must route through `stage_session_dsl_input` with the
corresponding MeerkatMachineInput variants. **Shell-only migration, no DSL change.**

**`PeerInput.payload: Option<serde_json::Value>`**
Added by PR #255 (bridge), not realtime. Already a plain input field on the
`PeerInput` struct. Fine as-is.

### E. Test/harness regressions

- `register_session_with_executor` is now `self: &Arc<Self>`. Tests calling it on
  `&MeerkatMachine` need `Arc::new(adapter).register_session_with_executor(...)`.
- `reconfigure_live_topology` tests (6 test functions) need the method to exist
  on `MeerkatMachine` — will work once Phase 3 lands it.
- `CommsDrainPhase`: tests import from `meerkat_core::comms_drain_lifecycle_authority`
  but path moved to `meerkat_runtime::CommsDrainPhase`. Shell import fix.

## The five phases

### Phase 1 — Extend MeerkatMachine DSL with realtime-attachment state

Edit `meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs`.

**Add state fields** (to the `state { ... }` block):
```
realtime_intent_present: bool,
realtime_binding_state: String,            // "Unbound" | "BindingNotReady" | "BindingReady" | "ReplacementPending"
realtime_binding_authority_epoch: Option<u64>,  // None when Unbound
realtime_reattach_required: bool,
realtime_next_authority_epoch: u64,
```

**Init block additions:**
```
realtime_intent_present = false,
realtime_binding_state = "Unbound",
realtime_binding_authority_epoch = None,
realtime_reattach_required = false,
realtime_next_authority_epoch = 1,
```

**Add input variants** (to `MeerkatMachineInput`):
```
ProjectRealtimeIntent { present: bool },
BeginRealtimeBinding,
ReplaceRealtimeBinding,
DetachRealtimeBinding,
RequireRealtimeReattach,
PublishRealtimeSignal { authority_epoch: u64, next_binding_state: String },
```

**Add effect variants** (to `MeerkatMachineEffect`):
```
RealtimeIntentProjected { present: bool },
RealtimeBindingRotated { authority_epoch: u64, binding_state: String },
```
(These are not strictly required for correctness — commands can satisfy shell
observability — but emitting explicit effects matches the pattern used by
other realtime-neutral DSL transitions.)

**Add transitions** (one per input above). Guards and updates:

- `ProjectRealtimeIntent { present }` — no guard; `update { self.realtime_intent_present = present; }`; `emit RealtimeIntentProjected { present }`.
- `BeginRealtimeBinding` — no guard; `update { self.realtime_binding_state = "BindingNotReady"; self.realtime_binding_authority_epoch = Some(self.realtime_next_authority_epoch); self.realtime_next_authority_epoch = self.realtime_next_authority_epoch + 1; self.realtime_reattach_required = false; }`; `emit RealtimeBindingRotated`.
- `ReplaceRealtimeBinding` — symmetric: `binding_state = "ReplacementPending"` + epoch rotation + `reattach_required = false`.
- `DetachRealtimeBinding` — no guard; `binding_state = "Unbound"; authority_epoch = None; reattach_required = false; next_epoch += 1`.
- `RequireRealtimeReattach` — no guard; `binding_state = "Unbound"; authority_epoch = None; reattach_required = true; next_epoch += 1`.
- `PublishRealtimeSignal { authority_epoch, next_binding_state }` — guard: `self.realtime_binding_authority_epoch == Some(authority_epoch) AND next_binding_state ∈ {"BindingNotReady", "BindingReady", "ReplacementPending"}`; `update { self.realtime_binding_state = next_binding_state; self.realtime_reattach_required = false; }` (epoch unchanged).

**Run codegen + verification:**
```
cargo xtask machine-codegen --all
cargo xtask machine-check-drift --all
cargo xtask machine-verify --all
```

This produces new kernel code, new TLA+ specs, and bounded TLC verification
that the model's guards hold. Any guard bug surfaces here.

### Phase 2 — Rewire shell through DSL

1. **Delete** `meerkat-runtime/src/meerkat_machine/realtime_attachment.rs`
   (the shell authority struct).
2. **Remove** `realtime_attachment: RuntimeRealtimeAttachmentAuthority` from
   `RuntimeSessionEntry` in `meerkat-runtime/src/meerkat_machine/mod.rs`.
3. **Rewrite** the 7 public methods in `session_management.rs` (or a new
   `realtime_attachment.rs` that is now pure shell-routing) so every mutation
   routes through `apply_dsl_input`/`stage_session_dsl_input`:
   - `project_realtime_attachment_intent(session_id, present)` → ProjectRealtimeIntent
   - `attach_live(session_id)` → BeginRealtimeBinding, then read DSL state to construct `RealtimeAttachmentSignalAuthority { session_id, authority_epoch: dsl.realtime_binding_authority_epoch.expect("set by transition") }`
   - `replace_realtime_attachment(session_id)` → ReplaceRealtimeBinding, same projection-back-to-token pattern
   - `detach_live(session_id)` → DetachRealtimeBinding
   - `require_realtime_attachment_reattach(session_id)` → RequireRealtimeReattach
   - `publish_realtime_attachment_signal(authority, status)` → PublishRealtimeSignal (DSL guard does the authority-epoch check; shell surfaces `RuntimeDriverError::ValidationFailed` on guard rejection)
   - `require_realtime_attachment_reattach_for_authority(authority)` → first do a state-read to validate authority, then RequireRealtimeReattach
4. **Rewrite** the two command arms in `dispatch_session.rs`:
   - `RuntimeRealtimeAttachmentStatus`: read DSL state fields, compute `RealtimeAttachmentStatus` via a small `impl` on `MeerkatMachineAuthority` or inline projection.
   - `IngestRealtimeAttachmentSignal`: no-op (the `publish_realtime_attachment_signal` shell method already applied DSL PublishRealtimeSignal). Consider whether to keep this command at all — might be cleaner to remove it.

### Phase 3 — Port runtime-owned orchestration methods

Add to `meerkat-runtime/src/meerkat_machine/session_management.rs` (or a new
`realtime_topology.rs` submodule):

- `reconfigure_live_topology(authority, request)` — full port from
  `/tmp/realtime-ports/meerkat_machine.rs.full:3970-4060`, unchanged except
  each internal DSL mutation now goes through the Phase-2 shell methods (which
  route through DSL), not the old shell-only authority.
- `revoke_realtime_attachment_ingress(authority) → Snapshot` — rotate the
  binding via ReplaceRealtimeBinding or RequireRealtimeReattach, capture the
  pre-rotation state for undo.
- `drive_live_topology_boundary(session_id)` — shell poll over DSL state until
  no active run. Wraps the `CallFinished` signal or uses the existing run
  completion waiter.
- `restore_realtime_attachment_authority(session_id, snapshot)` — re-apply the
  saved state via DSL inputs to put the binding back where it was pre-attempt.
- `fail_live_topology_after_detach(host, session_id, error)` — apply
  RequireRealtimeReattach + return the host error.

### Phase 4 — Other absorbed-authority shell cleanups

1. **`meerkat-runtime/src/input_ledger.rs`**: replace `state.apply(InputLifecycleInput::...)` call with the matching `stage_session_dsl_input(MeerkatMachineInput::...)`. Inputs to map: whatever the `input_ledger` code was using (QueueAccepted, StageForRun, MarkApplied, etc. — check the DSL's existing absorbed-input list).
2. **`CommsDrainPhase` imports**: the enum is re-exported from `meerkat_runtime::` (verified by grep earlier). Fix the stale `meerkat_core::comms_drain_lifecycle_authority::CommsDrainPhase` imports in test files.
3. **`maybe_spawn_comms_drain`** port: the method exists in the backup, port as-is onto `Arc<MeerkatMachine>`.

### Phase 5 — Tests

1. **`register_session_with_executor` call-site migration**: tests currently call on `&MeerkatMachine`, method is now `self: &Arc<Self>`. Either wrap with `Arc::new(...)` or use the existing `ensure_session_with_executor` pattern — whichever produces a cleaner diff.
2. **Realtime attachment tests** (6 functions in `meerkat_machine_tests.rs`): verify they pass against DSL-routed implementations. Expected to Just Work if Phase 2 is correct, since the public surface is unchanged.
3. **`reconfigure_live_topology` tests**: port whatever assertions the backup has.
4. **`JoinHandle<_>` type annotation** in one test — trivial.

## Verification gates

After each phase:
- Phase 1: `cargo xtask machine-codegen --all && cargo xtask machine-check-drift --all && cargo xtask machine-verify --all` — proves DSL model is well-formed and guards hold.
- Phase 2: `cargo check -p meerkat-runtime --all-features` — proves shell rewiring compiles.
- Phase 3: `cargo check --workspace --all-features` — proves orchestration methods wire up.
- Phase 4: `cargo check --workspace --all-features --tests` — proves shell cleanups are complete.
- Phase 5: `cargo unit && cargo int` at minimum; ideally `cargo e2e-fast` and the realtime lanes (`e2e-live`, `e2e-smoke` s71/s72).

## Dogma audit (self-check before touching code)

Against the short-form runtime dogma:

1. ✅ One semantic fact, one owner — realtime attachment state now has exactly one owner (MeerkatMachine DSL).
2. ✅ Machines own semantics — the binding-state machine, authority-epoch guard, and terminal-status rejection all live in DSL transitions.
3. ✅ Shell owns mechanics, not meaning — shell spawns tasks, composes DSL calls, surfaces results; it does not re-derive binding state.
4. ✅ One semantic condition, one terminal path — status projection is derived from DSL fields in one place.
5. ✅ Typed truth, never string folklore — `RealtimeAttachmentStatus` is a typed enum, not a string parse. (The DSL `binding_state` field is a string inside the DSL kernel because that's how the DSL models enum-shaped state; it projects back to the typed status enum at the shell boundary.)
6. ✅ App-facing APIs expose domain handles — `RealtimeAttachmentSignalAuthority { session_id, authority_epoch }` is a typed token.
7. ✅ Raw infra identity must be canonical — session_id is canonical.
8. ✅ `Option` must not hide ownership uncertainty — `realtime_binding_authority_epoch: Option<u64>` has clear semantics: Some when binding is live, None when Unbound.
9. N/A — tri-state overrides.
10. ✅ Dynamic policy follows dynamic identity — realtime attachment authority rotates on every binding mutation; status projection recomputes.
11. ✅ Derived projections are rebuildable, never authoritative — status projection is derived; authority-epoch rotation is authoritative.
12. ✅ Surfaces are skins, not authorities — REST/RPC surfaces query DSL via the status command.

## Open questions for Luka

1. **Effects for realtime transitions**: do we want explicit `RealtimeIntentProjected` / `RealtimeBindingRotated` DSL effects, or is the current shell-side observability enough? Effects cost nothing to add and improve the formal model's expressiveness.
2. **`IngestRealtimeAttachmentSignal` command**: currently a post-facto dispatch marker. After Phase 2, it's a no-op — should we delete the command variant entirely?
3. **TLC model bounds**: the default small-world (2-3 sessions) should be enough to cover realtime-attachment transitions, which are per-session. Confirm this is OK or ask for expanded bounds.
4. **Serial remote destroys** (unrelated mechanical regression from earlier rebase step): the rebase made `destroy_remote_members_for_destroy` sequential instead of `FuturesUnordered` because `dispose_member` needs `&mut self` under DSL. Acceptable or needs parallelism restored via some shared-state refactor?

## Out-of-scope for this port

- DSL-ifying the inner mechanics of `reconfigure_live_topology` itself (it's a shell composition; trying to model the multi-step orchestration in DSL would be a different project).
- Removing the vestigial `runtime-adapter` feature flag (separate cleanup; currently expands to nothing but I've kept the cfg-gates).
