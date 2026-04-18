# Realtime attachment + live-topology reconfigure

Load this reference when working on realtime attachment state, the
live-topology reconfigure flow, provider callback authority epochs, or
the peer-response-terminal context append path.

User-facing companion: `docs/guides/realtime.mdx`. That guide is the
source of truth for the public attach/detach surface, the state enum
table, and the authority-epoch contract. This reference covers the
internal DSL fields, invariants, and shell↔DSL routing used when
modifying the implementation.

## Public vocabulary

Public API surfaces describe `realtime`, not `voice`. Converged terms:

- `runtime/realtime_attachment_status` — runtime-owned status projection
- `mob/realtime_attach` / `mob/realtime_detach` — per-member attach/detach
- `mob/member_status.realtime_attachment_status` — per-member projection

`RealtimeAttachmentStatus` enum variants (runtime-owned):

- `Unattached` — session not registered or no intent present
- `IntentPresentUnbound` — operator requested live; binding not live yet
- `BindingNotReady` — binding exists; provider hasn't confirmed readiness
- `BindingReady` — provider reports realtime transport ready
- `ReplacementPending` — new authority minted; old binding draining
- `ReattachRequired` — prior binding invalidated; caller must reattach

## DSL state (catalog + runtime, both in sync)

`MeerkatMachineState`:

```
realtime_intent_present: bool,              // projected from mob voice intent
realtime_binding_state: String,             // "Unbound" | "BindingNotReady" | ...
realtime_binding_authority_epoch: Option<u64>,
realtime_reattach_required: bool,
realtime_next_authority_epoch: u64,         // monotonic; rotates on bind/detach
live_topology_phase: String,                // "Idle" | "Reconfiguring" | "Detached" | ...
```

`MobMachineState`:

```
member_voice_intent: Set<AgentIdentity>,    // survives respawn
```

### Invariants

- **`realtime_binding_epoch_consistency`** (MeerkatMachine):
  `(binding_state == "Unbound") == (authority_epoch == None)`.
  `Unbound+Some(epoch)` and `BindingReady+None` are TLC-unreachable.
- **Monotonic authority epoch**: `realtime_next_authority_epoch` only
  ever increments; `Some(e)` → `Some(e')` requires `e' > e`.
- **Topology-idle cross-guard**: realtime binding mutations
  (`BeginRealtimeBinding`, `ReplaceRealtimeBinding`,
  `PublishRealtimeSignal`) require `live_topology_phase == "Idle"`.
  Prevents provider callbacks from racing an in-progress LLM identity
  swap.

## DSL inputs (MeerkatMachine)

Realtime binding:
- `ProjectRealtimeIntent { present }` — shell reconciler from mob
- `BeginRealtimeBinding` — mint new authority epoch
- `ReplaceRealtimeBinding` — swap while preserving intent
- `DetachRealtimeBinding` — drop binding, keep intent
- `RequireRealtimeReattach` — invalidate, require fresh attach
- `PublishRealtimeSignal { authority_epoch, next_binding_state }` — provider callback

Live-topology reconfigure:
- `BeginLiveTopologyReconfigure { authority_epoch }`
- `MarkLiveTopologyDetached` — guard: `turn_phase ∈ {Ready, DrainingBoundary, Completed, Failed, Cancelled}`
- `ApplyLiveTopologyIdentity` / `ApplyLiveTopologyVisibility`
- `CompleteLiveTopology` — phase back to Idle
- `AbortLiveTopologyBeforeDetach` — binding preserved, caller may retry
- `FailLiveTopologyAfterDetach` — binding gone, reattach required

## RealtimeAttachmentSignalAuthority (token, not a state machine)

```rust
pub struct RealtimeAttachmentSignalAuthority {
    pub session_id: SessionId,
    pub authority_epoch: u64,
}
```

Minted by `BeginRealtimeBinding` / `ReplaceRealtimeBinding` transitions.
Provider presents it on every callback; DSL guard on
`PublishRealtimeSignal` validates `authority_epoch ==
realtime_binding_authority_epoch`. Stale tokens are rejected.

No `apply()` method, no transitions — it's a token.

## Public methods (meerkat-runtime)

On `MeerkatMachine`:

- `project_realtime_attachment_intent(&self, &SessionId, bool)` — shell
  reconciler from mob `MemberVoiceIntentSet`/`MemberVoiceIntentCleared`
- `attach_live(&self, &SessionId) -> Result<RealtimeAttachmentSignalAuthority, ...>` —
  gated on live executor; mints authority
- `replace_realtime_attachment(&self, &SessionId) -> ...` — same gate
- `detach_live(&self, &SessionId) -> ...`
- `require_realtime_attachment_reattach(&self, &SessionId) -> ...`
- `require_realtime_attachment_reattach_for_authority(&self, authority) -> ...` —
  only if caller still presents current authority
- `publish_realtime_attachment_signal(&self, authority, status) -> ...` —
  routes through DSL `PublishRealtimeSignal` with epoch validation
- `reconfigure_live_topology(&self, authority, SessionLlmReconfigureRequest) -> Result<new_authority, ...>` —
  orchestrates the full 6-step DSL flow (see below)

## Live-topology reconfigure orchestration

Implementation: `meerkat-runtime/src/meerkat_machine/llm_reconfigure.rs`.

Phases:

1. `BeginLiveTopologyReconfigure { authority_epoch }` — DSL rejects if
   `live_topology_phase != Idle` or epoch stale.
2. Eager `cancel_after_boundary_inner` — drives any in-flight turn to
   `DrainingBoundary` via control plane. Gated on control-channel
   presence; idempotent.
3. `prepare_reconfigure_session_llm_command` — host hydrate + resolve
   target. Fails pre-detach → `AbortLiveTopologyBeforeDetach` (binding
   preserved, retry OK).
4. Retry loop on `MarkLiveTopologyDetached` — DSL guard
   `turn_at_safe_boundary` blocks until turn reaches a safe phase.
   `meerkat_core::time_compat::Instant` + `crate::tokio::time::sleep`
   for wasm-safety.
5. Host `apply_live_session_llm_identity` + `ApplyLiveTopologyIdentity`.
   Failure past step 4 → `FailLiveTopologyAfterDetach` (binding gone,
   reattach required).
6. Host `apply_live_session_tool_visibility_state` + persist +
   `ApplyLiveTopologyVisibility`; then `CompleteLiveTopology`; then
   `attach_live` to mint a fresh authority for the caller.

## Context-only staged primitive routing (peer_response_terminal)

`peer_response_terminal` inputs carry `ConversationContextAppend` but
no conversation appends. Policy maps them to Steer → runtime boundary
`RunCheckpoint`, so `RunPrimitive::is_context_only_immediate()` (which
requires `boundary == Immediate`) does NOT match.

All five `CoreExecutor`-implementing apply entry points gate context
routing on the correct semantic precondition (no appends + has
context_appends), not on boundary:

- `meerkat-rpc/src/session_executor.rs::SessionRuntimeExecutor::apply`
- `meerkat-rpc/src/session_executor.rs::MobRpcRuntimeExecutor::apply`
- `meerkat-mob/src/runtime/provisioner.rs::MobSessionRuntimeExecutor::apply`
- `meerkat-mcp-server/src/runtime_ingress.rs::dispatch_primitive`
- `meerkat-rest/src/lib.rs::apply_runtime_turn`

Each short-circuits to `apply_runtime_context_appends` on the session
service, which writes through `agent.apply_runtime_system_context` →
`session.append_system_context_blocks`, appending
`[Runtime System Context]\nsource: peer_response_terminal:{peer}:{req}\n\n{text}`
to the first System message. Later turns (including reopened realtime
channels) see the token/intent durably.

## Shell ↔ DSL routing

All realtime public methods in `meerkat-runtime` stage DSL inputs via
`stage_session_dsl_input(&session_id, dsl::MeerkatMachineInput::...,
"context")`. None mutate shell state directly. Provider callback
validation happens inside the DSL guard, not in shell pre-checks.

## Known limitations (out of realtime port scope)

- **Inert `MobMachineState.member_voice_intent`**: `MobActor`'s
  `dsl_authority` isn't behind an interior-mutability primitive, so
  `handle_realtime_attach`/`detach` (which take `&self`) can't stage
  `RealtimeAttach`/`RealtimeDetach` DSL inputs. The shell roster's
  `voice_intent_present` is the current source of truth; DSL field
  tracks empty. Parity evaluator projects roster truth into the
  `member_voice_intent` key for coverage. Full fix requires wrapping
  `MobActor.dsl_authority` in `Arc<Mutex<_>>` (mirror MeerkatMachine).
- **Catalog-runtime DSL divergence on `MarkLiveTopologyDetached`**:
  catalog guards on `current_run_id == None`; runtime guards on
  `turn_phase ∈ {Ready, DrainingBoundary, ...}`. Catalog doesn't model
  `turn_phase` and is a strict over-approximation; TLC-proven
  invariants still hold in production.

## Key files

- `meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs` — catalog DSL
- `meerkat-machine-schema/src/catalog/dsl/mob_machine.rs` — catalog DSL
- `meerkat-runtime/src/meerkat_machine/dsl.rs` — runtime DSL (production)
- `meerkat-runtime/src/meerkat_machine/session_management.rs` — public methods
- `meerkat-runtime/src/meerkat_machine/llm_reconfigure.rs` — reconfigure_live_topology
- `meerkat-runtime/src/meerkat_machine/dispatch_control.rs` — RuntimeRealtimeAttachmentStatus projection
- `meerkat-runtime/src/meerkat_machine_types.rs` — `RealtimeAttachmentSignalAuthority`, `RealtimeAttachmentStatus`
- `meerkat-runtime/src/meerkat_machine_types.rs::SessionLlmReconfigureHost` — shell host trait
- `docs/architecture/identity-first-live-voice-proposal.md` — design notes
