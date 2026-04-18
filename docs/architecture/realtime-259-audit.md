# Phase 0 Audit — realtime-voice additions vs merge base cfe85b55a

Date: 2026-04-17 | Branch: `backup/realtime-voice-pre-259-rebase`
Method: `git diff cfe85b55a..backup -- <dir>`

## Summary

| File | Added | Classification |
|---|---:|---|
| `meerkat-runtime/src/meerkat_machine.rs` | ~720 | **DSL extension needed** — `RuntimeRealtimeAttachmentAuthority` (per-session state machine), 7 public methods, 5 topology helpers |
| `meerkat-runtime/src/meerkat_machine_types.rs` | <30 | Shell: `RealtimeAttachmentStatus` enum + `RealtimeAttachmentSignalAuthority` token type + 2 command variants. Projection types; command variants are dispatch envelopes. No DSL change. |
| `meerkat-runtime/src/input.rs` | 24 | Shell: `PeerInput.payload: Option<serde_json::Value>` field. Plain input data, not guarded state. No DSL change. |
| `meerkat-runtime/src/input_ledger.rs` | 63 | Shell: idempotency-dedup retention (TTL + limit). Shell mechanic. No DSL change. One test uses stale `InputState::apply(InputLifecycleInput::...)` API — Phase 4 fix. |
| `meerkat-runtime/src/runtime_loop.rs` | 343 | Shell: peer-response-terminal prompt rendering + context-append conversion. Pure formatting. No DSL change. |
| `meerkat-runtime/src/accept.rs` | 27 | Shell: `RoutingDisposition::Immediate` → `HandlingMode::Steer` mapping. Policy→shell translation. No DSL change. |
| `meerkat-runtime/src/policy_table.rs` | 34 | Shell policy table: `peer_response_terminal` moved from `StageRunStart+Queue` to `InjectNow+Immediate+Steer`. Shell table update. |
| `meerkat-runtime/src/silent_intent.rs` | 3 | Shell: trivial helper tweak. |
| `meerkat-runtime/src/peer_handling_mode.rs` | 8 | Shell: trivial helper. |
| `meerkat-runtime/src/comms_drain.rs` | 2170 | Shell: bridge protocol transport (PR #255 content — BridgeCommand/Reply dispatch, auth-exempt gating, canonicalize_bridge_address, etc.). Dispatches via existing MeerkatMachineCommand::SetPeerIngressContext. No new semantic state. |
| `meerkat-runtime/src/comms_bridge.rs` | 23 | Shell: `peer_payload` helper extracting payload from interaction. No DSL change. |
| `meerkat-runtime/src/service_ext.rs` | 10 | Shell: trait adds `realtime_attachment_status(session_id)` method. Projection read via a new command. No DSL change per se. |
| `meerkat-runtime/src/coalescing.rs`, `durability.rs` | 5 ea | Shell: test updates for the payload field. |
| `meerkat-mob/src/event.rs` | 54 | Shell: MemberRef.External adds `bootstrap_token` (PR #255 bridge). Data carrier. No DSL change. |
| `meerkat-mob/src/roster.rs` | 123 | **DSL extension needed** — `voice_intent_present: bool` per-RosterEntry field, driven by `MemberVoiceIntentSet/Cleared` events. Shell-only semantic state on a mob fact. |
| `meerkat-mob/src/runtime/actor.rs` | ~500 | Mostly shell mechanics, but includes `MobCommand::RealtimeAttach/Detach` handling and `reconcile_realtime_attachment_runtime` which is a composition seam between mob DSL (voice intent) and runtime DSL (realtime attachment). |
| `meerkat-mob/src/runtime/builder.rs` | ~60 | Shell: `restore_realtime_attachment_intent_if_needed` during mob resume. Composition-seam reconciliation helper. |
| `meerkat-mob/src/runtime/handle.rs` | ~60 | Shell: `MobRealtimeAttachmentStatus` projection type + query method. Projection read. No DSL change. |
| `meerkat-mob/src/runtime/provisioner.rs` | ~50 | Shell: bridge integration. |
| `meerkat-mob/src/runtime/session_service.rs` | ~50 | Shell: session service bridge wiring. |
| `meerkat-client/src/openai_live.rs`, `openai_realtime_attachment.rs`, `realtime_session.rs` | large | Shell: OpenAI realtime SDK + meerkat attachment driver. Pure provider transport + shell reconciliation. No DSL change. |
| `meerkat-core/src/agent.rs`, `agent/runner.rs`, `agent/state.rs`, `session.rs` | small | Shell: minor ergonomics. No new machine state. |

## DSL extensions required (refined scope)

### A. MeerkatMachine DSL
1. **Realtime attachment authority** (Phase 1 in port plan):
   - State: `realtime_intent_present: bool`, `realtime_binding_state: String`, `realtime_binding_authority_epoch: Option<u64>`, `realtime_reattach_required: bool`, `realtime_next_authority_epoch: u64`.
   - Inputs: `ProjectRealtimeIntent`, `BeginRealtimeBinding`, `ReplaceRealtimeBinding`, `DetachRealtimeBinding`, `RequireRealtimeReattach`, `PublishRealtimeSignal` (authority-epoch guard).

2. **Live-topology reconfigure state** (Phase 3 in port plan):
   - State: `live_topology_phase: String`.
   - Inputs: `BeginLiveTopologyReconfigure`, `MarkLiveTopologyDetached`, `ApplyLiveTopologyIdentity`, `ApplyLiveTopologyVisibility`, `CompleteLiveTopology`, `AbortLiveTopologyBeforeDetach`, `FailLiveTopologyAfterDetach`.
   - Cross-guards: `PublishRealtimeSignal`/`BeginRealtimeBinding`/`ReplaceRealtimeBinding` require `live_topology_phase == "Idle"`.

### B. MobMachine DSL (NEW — not in original plan)
3. **Per-member voice intent** (new Phase 1b):
   - State: `member_voice_intent: Map<String, bool>` — keyed on `AgentIdentity.as_str()`, indicating durable operator intent for live voice on that member.
   - Inputs: `SetMemberVoiceIntent { identity: String }`, `ClearMemberVoiceIntent { identity: String }`. Guarded by `member_present[identity] == true` (re-use existing MobMachine member-presence field).
   - Effects: `VoiceIntentSet { identity }`, `VoiceIntentCleared { identity }` — used for event persistence and runtime reconciliation triggers.
   - Shell integration: the existing `MemberVoiceIntentSet`/`MemberVoiceIntentCleared` persisted events replay as DSL inputs during resume. `handle_realtime_attach`/`handle_realtime_detach` in `MobActor` become thin dispatchers into the DSL.

### C. Composition seam (cross-machine)
`reconcile_realtime_attachment_runtime` is a meerkat-mob shell method that, upon a MobMachine voice-intent transition, drives the runtime MeerkatMachine's realtime-attachment transitions through the runtime adapter handle. This is a legitimate composition seam (mob → runtime), not DSL state on its own. Formalising it as a composition protocol is optional but desirable; for this port it stays as shell code that calls the two DSLs in the correct order.

## Shell mechanics that stay

- OpenAI provider transport (`meerkat-client/src/openai_live.rs`, etc.) — provider-specific IO.
- Comms drain task spawning (`maybe_spawn_comms_drain`) — routes through existing `SetPeerIngressContext` DSL command.
- Prompt rendering (`peer_prompt_text` in runtime_loop.rs) — pure formatting.
- Policy table (`policy_table.rs`) — shell lookup, resolves into DSL inputs at admission.
- Idempotency dedup retention (`input_ledger.rs`) — shell mechanic.

## Shell cleanups (Phase 4 in port plan)

- `input_ledger.rs:276` test — replace `state.apply(InputLifecycleInput::ConsumeOnAccept)` with direct `state.terminal_outcome = Some(...)` (DSL has absorbed that input, shell test should not simulate DSL transitions).
- `CommsDrainPhase` imports in `meerkat-mob/src/runtime/tests.rs` and `meerkat-runtime/src/meerkat_machine_tests.rs` — re-export from `meerkat_runtime` instead of the deleted `meerkat_core::comms_drain_lifecycle_authority` path.
- `runtime-adapter` feature flag removal (~101 cfg sites in meerkat-mob).
- `destroy_remote_members_for_destroy` parallelism restore (make dispose_member &self via interior mutability).

## Net impact on port plan

Original plan had 5 phases + Phase 0 audit. Audit reveals an additional DSL extension in MobMachine for per-member voice intent. Updated phase list:

- Phase 0: Audit (this doc).
- Phase 1a: MeerkatMachine DSL realtime attachment state.
- Phase 1b (new): MobMachine DSL per-member voice intent state.
- Phase 2: Shell rewiring over both DSLs.
- Phase 3: MeerkatMachine DSL live-topology reconfigure + shell drivers.
- Phase 4: Absorbed-authority cleanups + feature-flag removal + parallelism restore.
- Phase 5: Tests + lane verification.

Phases 1a and 1b can be done in the same DSL editing session (both edit catalog DSL files, run codegen/verify together). Phase 3 requires its own codegen/verify cycle because the topology-phase guards reference Phase 1a fields.
