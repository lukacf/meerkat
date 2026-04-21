# Realtime attachment + live-topology reconfigure

Load this reference when working on realtime attachment state, the
live-topology reconfigure flow, provider callback authority epochs, or
the peer-response-terminal context append path.

User-facing companion: `docs/guides/realtime.mdx`. That guide is the
source of truth for the capability-driven public model (choose a
realtime-capable model; transport attaches automatically), the state
enum table, and the authority-epoch contract. This reference covers the
internal DSL fields, invariants, and shell↔DSL routing used when
modifying the implementation.

## Capability-driven public surface

Realtime is a delivery mode of the session's LLM, not a separate
subsystem. `ModelCapabilities.realtime: bool` (defined in
`meerkat-models/src/capabilities/mod.rs`, projected onto
`ModelProfile.realtime` in `meerkat-models/src/profile/mod.rs`) drives
all transport attach/detach decisions. There is no caller-initiated
attach/detach RPC.

Public vocabulary:

- `runtime/realtime_attachment_status` — runtime-owned status projection (single session)
- `runtime/realtime_attachment_statuses` — batch projection
- `realtime/open_info` / `realtime/status` / `realtime/capabilities` — product-layer bootstrap

Previously public methods, now deleted:

- `mob/realtime_attach` / `mob/realtime_detach` — removed (T6b)
- `MobHandle::realtime_attach` / `realtime_detach` — removed (T5h)
- `RealtimeChannelTarget::MobMemberTarget` — removed (T5i); callers pass `SessionTarget` directly
- `MemberVoiceIntentSet` / `MemberVoiceIntentCleared` events — deprecated, replay as no-ops (T6c)

Runtime-internal methods still named `attach_live` / `detach_live` /
`reconfigure_live_topology` on `MeerkatMachine` — these are called by
the capability-driven policy (below), not by remote callers.

`RealtimeAttachmentStatus` enum variants (runtime-owned):

- `Unattached` — session's resolved model is not realtime-capable
- `IntentPresentUnbound` — runtime has begun bringing up a binding
- `BindingNotReady` — binding exists; provider hasn't confirmed readiness
- `BindingReady` — provider reports realtime transport ready
- `ReplacementPending` — new authority minted (e.g. during reconfigure); old binding draining
- `ReattachRequired` — prior binding invalidated by a post-detach failure; reconfigure required

## DSL state (catalog + runtime, both in sync)

`MeerkatMachineState`:

```
realtime_binding_state: String,             // "Unbound" | "BindingNotReady" | ...
realtime_binding_authority_epoch: Option<u64>,
realtime_reattach_required: bool,
realtime_next_authority_epoch: u64,         // monotonic; rotates on bind/detach
live_topology_phase: String,                // "Idle" | "Reconfiguring" | "Detached" | ...
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
- `BeginRealtimeBinding` — mint new authority epoch
- `ReplaceRealtimeBinding` — swap while preserving session identity
- `DetachRealtimeBinding` — drop binding
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

## Capability-driven transport policy

Implementation: `meerkat-runtime/src/meerkat_machine/llm_reconfigure.rs`
(`apply_capability_driven_realtime_transport`).

At session-init time and at the final step of `reconfigure_live_topology`,
the runtime hydrates `SessionLlmCapabilitySurface.realtime` (populated in
`meerkat-rpc/src/session_runtime.rs::profile_to_capability_surface` from
the resolved `ModelProfile`) and branches:

- Target is realtime-capable and current binding is absent → call
  `attach_live` internally (mints authority epoch via
  `BeginRealtimeBinding`).
- Target is not realtime-capable and a binding exists → call
  `detach_live` internally (via `DetachRealtimeBinding`).
- Target is realtime-capable and a binding exists on a different identity
  → during reconfigure, the `Detached` step already dropped the binding;
  the capability branch calls `attach_live` to mint a fresh authority.

There is no caller-initiated attach path.

## Public methods (meerkat-runtime, internal to host composition)

On `MeerkatMachine` (called by host/facade code, not exposed as RPC):

- `attach_live(&self, &SessionId) -> Result<RealtimeAttachmentSignalAuthority, ...>` —
  gated on live executor; mints authority. Called by the capability policy.
- `replace_realtime_attachment(&self, &SessionId) -> ...` — same gate.
- `detach_live(&self, &SessionId) -> ...`
- `require_realtime_attachment_reattach(&self, &SessionId) -> ...`
- `require_realtime_attachment_reattach_for_authority(&self, authority) -> ...` —
  only if caller still presents current authority.
- `publish_realtime_attachment_signal(&self, authority, status) -> ...` —
  routes through DSL `PublishRealtimeSignal` with epoch validation.
- `reconfigure_live_topology(&self, authority, SessionLlmReconfigureRequest) -> Result<Option<new_authority>, ...>` —
  orchestrates the full 6-step DSL flow; final step branches on
  target capability's `realtime` bit.
- `apply_capability_driven_realtime_transport(&self, &SessionId) -> ...` —
  session-init seam for the capability-driven policy.
- `realtime_attachment_status(&self, &SessionId)` — read the
  runtime-owned projection.

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
   final capability branch — if target is realtime-capable, call
   `attach_live` to mint a fresh authority; else return None and leave
   status `Unattached`.

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

All realtime methods in `meerkat-runtime` stage DSL inputs via
`stage_session_dsl_input(&session_id, dsl::MeerkatMachineInput::...,
"context")`. None mutate shell state directly. Provider callback
validation happens inside the DSL guard, not in shell pre-checks.

## Known limitations (out of realtime port scope)

- **Catalog-runtime DSL divergence on `MarkLiveTopologyDetached`**:
  catalog guards on `current_run_id == None`; runtime guards on
  `turn_phase ∈ {Ready, DrainingBoundary, ...}`. Catalog doesn't model
  `turn_phase` and is a strict over-approximation; TLC-proven
  invariants still hold in production.
- **OpenAI Realtime only**: `gpt-realtime-1.5` is the canonical production
  realtime-capable model today. Gemini `*-live*` models are reserved
  in the capability derivation but have no production entries.

## Key files

- `meerkat-models/src/capabilities/mod.rs` — `ModelCapabilities.realtime`
- `meerkat-models/src/profile/mod.rs` — `ModelProfile.realtime` (projection)
- `meerkat-models/src/profile/openai.rs` — `realtime: m.contains("realtime")`
- `meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs` — catalog DSL
- `meerkat-machine-schema/src/catalog/dsl/mob_machine.rs` — catalog DSL
- `meerkat-runtime/src/meerkat_machine/dsl.rs` — runtime DSL (production)
- `meerkat-runtime/src/meerkat_machine/session_management.rs` — public methods
- `meerkat-runtime/src/meerkat_machine/llm_reconfigure.rs` — reconfigure_live_topology + capability-driven transport
- `meerkat-runtime/src/meerkat_machine/dispatch_control.rs` — RuntimeRealtimeAttachmentStatus projection
- `meerkat-runtime/src/meerkat_machine_types.rs` — `RealtimeAttachmentSignalAuthority`, `RealtimeAttachmentStatus`, `SessionLlmCapabilitySurface.realtime`
- `meerkat-runtime/src/meerkat_machine_types.rs::SessionLlmReconfigureHost` — shell host trait
- `docs/architecture/identity-first-live-voice-proposal.md` — design notes
