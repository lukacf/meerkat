# SessionRuntime Split — surface-agnostic session orchestrator extraction

## Goal

Move the surface-agnostic session orchestration logic out of
`meerkat-rpc::session_runtime::SessionRuntime` into a shared crate
(`meerkat::session_runtime::MeerkatSessionRuntime` or similar) so
**every Meerkat surface** can compose it directly: Rust SDK (#1
surface), rkat-rpc (#2, backs Python/TS SDKs), CLI, REST,
MCP-server, web SDK, and embedded `examples/*`.

The current state inverts the architecture: `meerkat-cli` imports
`meerkat_rpc::session_runtime::SessionRuntime` because RPC is the only
crate that ever bundled the live-channel + staged-promotion + hot-swap
+ persistence-recovery glue. RPC ≠ #1 surface; the orchestrator
belongs upstream of RPC.

## Coordination — read this before touching anything

- **Other agents are active in this worktree.** Coordinate via this
  TODO. Never run `git checkout -- .`, `git reset --hard`, or
  `git stash drop` without checking the working tree and stash list
  for non-yours WIP.
- **`stash@{0}` (`codex-cloudbuild-bb-migration-wip-before-main-rebase`)
  belongs to a sibling worktree.** Never drop, pop, or apply it.
- **Pre-commit auto-fmt and pre-push clippy hold the workspace cargo
  lock.** Avoid parallel `cargo build` invocations across agents.
- **Avoid heavy `cargo build --workspace` commands.** Default to
  package-scoped checks (`./scripts/repo-cargo check -p <crate>`).
- **Add regression tests where appropriate** — every behavioural move
  needs at least one assertion that the seam still answers correctly
  in its new home.
- **Each adversarial verifier reports at most 5 issues per pass.**
  Do not accumulate.

## Phase model

The work is sequenced as a foundation phase, then three parallel
waves of moves, then a single rewire phase, then a meta-level
adversarial verification phase. Foundation MUST land before any wave;
waves may run in parallel (different files / structs).

```
Foundation   → Wave 1   ┐
                Wave 2   ├─→ Rewire   → Verify   → CI gate
                Wave 3   ┘
```

## Common-thread root causes from prior reviews — verifier seed

Past reviews of this PR kept missing the same four structural
patterns. The Phase-V verifier MUST hunt for these explicitly,
not for surface-level lint:

1. **Lane / dimension collapse.** Two semantically-distinct concerns
   sharing a single channel/struct/method (display vs spoken
   transcript; control vs lossy audio; advertised capability vs
   runtime config; command rejection vs terminal error).
2. **Identity loss across boundaries.** Internal layer stamps an
   identity; the public layer drops it (`response_id`, `item_id`,
   `session_id`, `bound_llm_identity`, …). Pay special attention to
   any conversion that flattens `Option<…>` to `()` or returns `String`
   instead of a typed newtype.
3. **Unhappy-path state gaps.** Happy path admits the session into
   the live map; error / interrupt / cancellation paths leave it
   half-bound (`status=Closed`, `retire_at=None`, adapter still held).
   Audit `?` propagation, `Err` arms, `match _ => Noop`.
4. **Two-path duplication.** Same fact computed in two places
   (session-stored model vs config-resolved model; precheck
   identity vs open_config identity; staged identity vs live
   identity). Always identify the canonical owner; the other site
   must derive, not duplicate.

## Findings

Convention: each finding has `[ ] fix` `[ ] verify` checkboxes. The
verifier is a **separate** agent from the implementer — never
self-verify.

### Phase 0 — Foundation (sequential, must land before any wave)

- [x] fix · [x] verify · **F1.** Add `meerkat-live` as an optional
  dep on the `meerkat` facade, gated by a new `live` feature
  (`meerkat/Cargo.toml`). Re-export `meerkat_live::*` under
  `meerkat::live` when the feature is enabled. Update CI feature
  matrix to cover the new feature axis. *Verify: `cargo check -p
  meerkat` and `cargo check -p meerkat --features live` both clean
  at d8e8220aa.*

- [x] fix · [x] verify · **F2.** Create
  `meerkat/src/session_runtime/mod.rs` with:
  - `pub struct MeerkatSessionRuntime` (empty shell, all
    fields pub(crate))
  - `pub struct SessionRuntimeBuilder` for constructor ergonomics
  - module hierarchy: `mod admission;`, `mod staged_promotion;`,
    `mod live_orchestration;`, `mod recovery;`, `mod runtime_state;`,
    `mod errors;` — each empty for now
  - Re-export from `meerkat::session_runtime::{MeerkatSessionRuntime,
    SessionRuntimeBuilder, SessionInfo, SessionState,
    LiveOpenPrecheckError}`
  *Verify: skeleton compiles; `meerkat::session_runtime::*` resolves
  from facade re-exports.*

- [x] fix · [x] verify · **F3.** Define a typed error taxonomy in
  `meerkat::session_runtime::errors` that does NOT depend on
  `meerkat_rpc::error::RpcError`. Use `thiserror`-derived enums with
  stable `code()` methods. Surfaces map these to their wire errors
  (RpcError, axum::Response, JS Error, etc.). Source the existing
  enum bodies from `meerkat-rpc/src/error.rs` and
  `LiveOpenPrecheckError` in `session_runtime.rs:130`. *Verify:
  variants and `code()` slugs are surface-agnostic; no RPC type in
  signature.*

- [x] fix · [x] verify · **F4.** Audit every symbol the W1/W2/W3
  moves will need. Listing below — the contract Wave 1/2/3 fulfills.
  Source line numbers are pinned at commit `b4b420d35`; subsequent
  diffs may re-number but the symbol identity is stable.

  **Pure types (W1 targets):**
  - `LiveOpenPrecheckError` (l.130) → `errors` (already extracted in F3)
  - `StagedAdmissionRestore` (l.199) → `admission`
  - `RecoveredCreateRequest` (l.210) → `recovery`
  - `RecoveryRuntimeBindingMode` (l.216) → `recovery`
  - `PendingSessionEventStreams` (l.222) → `runtime_state`
  - `ArchiveRuntimeCleanup` (l.228) + `impl` (l.238) → `runtime_state`
  - `PendingSessionEventStreamDrop` (l.294) + `Drop impl` (l.298) → `runtime_state`
  - `RuntimePreAdmissionGuard` (l.491, 533) → `admission`
  - `RuntimePreAdmissionEntry` (l.495) → `admission`
  - `RuntimePreAdmissionRegistration` (l.500, 545, 566 Drop) → `admission`
  - `RuntimeRegistrationLockLease` (l.507, 560, 575 Drop) → `admission`
  - `StagedArchiveRollbackGuard` (l.513, 594, 608 Drop) → `admission`
  - `RuntimePreAdmission` (l.632, 663 From, 669 Drop) → `admission`
  - `PendingPromotionCleanupMode` (l.828) → `staged_promotion`
  - `PendingPromotionCleanup` (l.833, 849 impl, 1062 Drop) → `staged_promotion`

  **Type aliases (W1):**
  - `ActiveCapacityGuard = meerkat::RuntimeContextAdmissionGuard` (l.195) → `admission`
  - `StagedCapacityAdmissions = Arc<StdMutex<HashMap<SessionId, ActiveCapacityGuard>>>` (l.196) → `admission`
  - `ServiceStartTurnResultReceiver` (l.187) → `staged_promotion`
  - `ServiceApplyRuntimeTurnResultReceiver` (l.189) → `staged_promotion`
  - `RecoverableServiceApplyRuntimeTurnResultReceiver` (l.191) → `staged_promotion`

  **Helper free functions (W2 targets):**
  - `precheck_identity` (l.149) → `live_orchestration`
  - `apply_precheck_gates` (l.167) → `live_orchestration`
  - `render_context_append_text` (l.84) → `runtime_state` (helper)
  - `pending_system_context_appends` (l.99) → `staged_promotion`
  - `extract_system_prompt_from_seed_messages_runtime` (l.329) → `live_orchestration`
  - `build_live_projection_snapshot_for_runtime` (l.351) → `live_orchestration`
  - `live_channel_requires_close_for_identity_change` (l.400) → `live_orchestration`
  - `realtime_projection_root_system_message` (l.410) → `live_orchestration`
  - `realtime_projection_messages` (l.450) → `live_orchestration`
  - `realtime_projection_runtime_system_context` (l.461) → `live_orchestration`
  - `exported_tool_visibility_state` (l.470) → `live_orchestration`
  - `builtin_tool_visibility_witness` (l.478) → `live_orchestration`
  - `parse_provider_override` (l.80) → `recovery` (used by `recovery_overrides_from_turn`)
  - `unknown_provider_message` (l.76) → `recovery`

  **SessionRuntime methods (W2 targets, all on `impl SessionRuntime`):**
  - Live orchestration (W2-A): `precheck_live_open`,
    `recover_live_session_for_realtime_open`,
    `materialize_staged_session_for_realtime_open`,
    `realtime_session_open_config`, `live_open_config_for_session`,
    `propagate_config_to_live_channels`
  - Recovery (W2-B): `load_persisted_session`, `recovered_create_request`,
    `recovered_create_request_with_runtime_binding_mode`,
    `recovery_overrides_from_turn`, `recovery_external_tools`,
    `cleanup_recovered_runtime_if_new`
  - Hot-swap (W2-C): `hot_swap_llm_client_on_idle_session`,
    `derive_reconfigured_visibility_state`,
    `rollback_idle_hot_swap_failure`, `SessionRuntimeLlmReconfigureHost`
    struct + impl (l.1922, 1932, 2170)
  - Staged-promotion lifecycle (W2-D):
    `spawn_pending_create_and_apply_runtime_turn_with_admission_guard`,
    `await_service_apply_runtime_turn`,
    `await_service_apply_runtime_turn_with_recoverable_admission`,
    `await_guarded_create_session`,
    `finish_pending_promotion_after_service_turn`,
    `pending_live_first_turn_is_still_deferred`,
    `should_restore_pending_after_start_turn`,
    `is_pre_run_apply_runtime_turn_failure`,
    `is_archived_create_rejection`
  - State observers (W2-E): `archived_persisted_session_without_live`,
    `live_session_is_stale`, `try_recover_persisted_session`,
    `discard_live_session`, `discard_stale_live_session`

  **Wave 3 surfaces:**
  - `SkillIdentityRegistryState` (l.2301) → `runtime_state`
  - `SessionMcpState` (l.2307) → `runtime_state`
  - `SessionState` (l.2322), `SessionInfo` (l.2344) → `runtime_state`
    (placeholder shells already in F2)
  - Accessors: `config_runtime`, `realm_id`, `instance_id`, `backend`,
    `default_llm_client`, `live_adapter_host`, `set_live_adapter_host`,
    `set_default_llm_client`, `live_tool_dispatcher`,
    `skill_identity_registry`, `set_skill_identity_registry_for_generation`,
    `build_skill_identity_registry`

  **Stays in `meerkat-rpc` (RPC-specific, do NOT move):**
  - `RpcMobSessionService` (l.1132, 1142, 1424, 1590, 1615, 1635, 1647)
  - `session_error_to_rpc`, `runtime_driver_error_to_rpc` (RPC error mappers)
  - `RpcEventPump` glue, `handlers/*` integration shims
  - The thin `SessionRuntime` shell that delegates to
    `Arc<MeerkatSessionRuntime>` (R1)

### Phase 1 — Wave 1: pure session-orchestration types (parallel by file)

- [x] fix · [x] verify · **W1-A.** Move
  `StagedCapacityAdmissions` (typedef + helpers
  `take_staged_capacity_admission`, `insert_staged_capacity_admission`,
  `has_staged_capacity_admission`, `discard_staged_capacity_admission`,
  `restore_staged_capacity_admission`) from
  `meerkat-rpc/src/session_runtime.rs` to
  `meerkat::session_runtime::admission`. Re-export from
  `meerkat-rpc::session_runtime` for backward compatibility.

- [x] fix · [x] verify · **W1-B.** Move `PendingPromotionCleanup`
  (struct + Drop impl + all 13 methods) from
  `meerkat-rpc/src/session_runtime.rs:828-1129` to
  `meerkat::session_runtime::staged_promotion`. The Drop impl spawns
  a tokio task — verify it still compiles in the new crate (tokio
  dep is already on `meerkat` facade).
  *Note:* the destination module is gated on
  `cfg(all(feature = "session-store", not(target_arch = "wasm32")))`
  because `PersistentSessionService` and
  `MachineSessionArchiveProtocol` are only compiled with that
  feature. `meerkat-rpc` already enables `session-store` so the
  re-export works unconditionally on its side.

- [x] fix · [x] verify · **W1-C.** Move `RuntimePreAdmission`,
  `RuntimePreAdmissionGuard`, `RuntimePreAdmissionEntry`,
  `RuntimePreAdmissionRegistration`, `RuntimeRegistrationLockLease`,
  `StagedArchiveRollbackGuard`, `StagedAdmissionRestore` from
  `meerkat-rpc/src/session_runtime.rs:491-680` to
  `meerkat::session_runtime::admission`. These are RAII guards with
  `Drop` impls — confirm Drop runs in the new crate's tokio runtime.
  *Deviation:* `RuntimePreAdmissionRegistration` deferred to W3-A. It
  holds an `Arc<SessionRuntime>` and dispatches a method on it from
  Drop, so it can't move without either a trait abstraction over the
  runtime or pulling `SessionRuntime` itself upstream — both broader
  than Wave 1's pure-type-move charter. The other six types
  (`StagedAdmissionRestore`, `RuntimePreAdmission`,
  `RuntimePreAdmissionGuard`, `RuntimePreAdmissionEntry`,
  `RuntimeRegistrationLockLease`, `StagedArchiveRollbackGuard`)
  moved cleanly with their Drop semantics intact.

- [x] fix · [x] verify · **W1-D.** Move `RecoveredCreateRequest` and
  `RecoveryRuntimeBindingMode` from `session_runtime.rs:210,216` to
  `meerkat::session_runtime::recovery`. These are pure data types;
  zero behavioural risk.

- [x] fix · [x] verify · **W1-E.** Move `ArchiveRuntimeCleanup`,
  `PendingSessionEventStreams`, `PendingSessionEventStreamDrop` from
  `session_runtime.rs:222-300` to
  `meerkat::session_runtime::runtime_state`. The Drop impl on
  `PendingSessionEventStreamDrop` and the cleanup logic both touch
  `runtime_adapter` — ensure the abstraction crosses crate bounds
  cleanly (`Arc<MeerkatMachine>` is in `meerkat-runtime`, already a
  facade dep).
  *Deviation:* `ArchiveRuntimeCleanup` deferred to W3-A. Its fields
  reference `SessionMcpState` (RPC-private) and
  `meerkat_mob_mcp::MobMcpState` (a crate that `meerkat` does not yet
  depend on). Moving it now would require generics + trait bounds
  (behaviour-changing) or new `meerkat` deps; both broader than the
  Wave-1 "pure type move" charter. Only `PendingSessionEventStreams`
  and `PendingSessionEventStreamDrop` moved.

### Phase 2 — Wave 2: surface-agnostic methods (parallel by area)

- [x] fix · [x] verify · **W2-A.** Move live-channel orchestration
  methods (`precheck_identity`, `apply_precheck_gates`,
  `precheck_live_open`, `recover_live_session_for_realtime_open`,
  `materialize_staged_session_for_realtime_open`,
  `realtime_session_open_config`, `live_open_config_for_session`,
  `propagate_config_to_live_channels`) from
  `session_runtime.rs:149-4305 + 5461-5705` to
  `meerkat::session_runtime::live_orchestration`. Gate with
  `#[cfg(feature = "live")]`. The `LiveAdapterHost` parameter remains
  injected; the orchestrator does not own it.

- [x] fix · [x] verify · **W2-B.** Move session-recovery helpers
  (`load_persisted_session`, `recovered_create_request`,
  `recovered_create_request_with_runtime_binding_mode`,
  `recovery_overrides_from_turn`, `recovery_external_tools`,
  `cleanup_recovered_runtime_if_new`) to
  `meerkat::session_runtime::recovery`. These are zero-RPC; they
  build `CreateSessionRequest` and `Session` values from durable
  store + runtime config.
  *Deviation:* `recovery_overrides_from_turn` (depends on the
  RPC-private `crate::handlers::turn::TurnOverrides`),
  `recovery_external_tools` (depends on the RPC callback dispatcher
  `crate::callback_dispatcher::CallbackToolDispatcher`), and
  `cleanup_recovered_runtime_if_new` (depends on the RPC-private
  `ArchiveRuntimeCleanup` whose move is deferred to W3-A) stay in
  `meerkat-rpc` until those upstream blockers clear. Moved cleanly:
  `parse_provider_override`, `unknown_provider_message`, and a new
  `RecoveryContext<'a>` orchestrator that owns the
  `load_persisted_session`, `recovered_create_request`, and
  `recovered_create_request_with_runtime_binding_mode` flows behind a
  surface-agnostic `RecoveryError` (variants
  `Recovery`/`BindingPreparation`/`Session`). The SessionRuntime
  shims remain RPC-typed (RpcError) and translate via a new
  `recovery_error_to_rpc` adapter.

- [x] fix · [x] verify · **W2-C.** Move LLM hot-swap surface
  (`hot_swap_llm_client_on_idle_session`, `hot_swap_llm_client`
  minus its `TurnOverrides` parameter shim,
  `derive_reconfigured_visibility_state`, `rollback_idle_hot_swap_failure`,
  `SessionRuntimeLlmReconfigureHost`) to
  `meerkat::session_runtime::live_orchestration`. The `TurnOverrides`
  shim stays in `meerkat-rpc` (it's the RPC surface adapter).

- [x] fix · [x] verify · **W2-D.** Move staged-session lifecycle
  helpers (`spawn_pending_create_and_apply_runtime_turn_with_admission_guard`,
  `await_service_apply_runtime_turn`,
  `await_service_apply_runtime_turn_with_recoverable_admission`,
  `await_guarded_create_session`,
  `finish_pending_promotion_after_service_turn`,
  `pending_live_first_turn_is_still_deferred`,
  `should_restore_pending_after_start_turn`,
  `is_pre_run_apply_runtime_turn_failure`,
  `is_archived_create_rejection`) to
  `meerkat::session_runtime::staged_promotion`. The `tokio::spawn`
  callsites remain identical; verify the spawned tasks have the same
  cancellation semantics in the new crate.

- [x] fix · [x] verify · **W2-E.** Move `SessionInfo`, `SessionState`
  enums and `archived_persisted_session_without_live`,
  `live_session_is_stale`, `try_recover_persisted_session`,
  `discard_live_session`, `discard_stale_live_session` to
  `meerkat::session_runtime::runtime_state`. These are session-state
  observers; surface-agnostic.
  *Deviation:* `archived_persisted_session_without_live` depends on
  the RPC-private `ArchiveRuntimeCleanup` (deferred to W3-A) and stays
  in `meerkat-rpc`. `try_recover_persisted_session` is a
  `#[cfg(test)]` helper that composes RPC-private `TurnOverrides` and
  `RpcError`; stays in place. The real `SessionState`/`SessionInfo`
  bodies replace the F2 placeholder enums in `runtime_state`. A new
  `RuntimeStateOps<'a>` orchestrator owns `discard_live_session`,
  `discard_stale_live_session`, and `live_session_is_stale`; SessionRuntime
  exposes a `runtime_state_ops()` builder and re-exports
  `SessionInfo`/`SessionState` from the moved module. `meerkat` gained
  a `serde` dep (with `derive`) so the typed `SessionState` can carry
  its `serde::Serialize`/`Deserialize` derives upstream.

### Phase 3 — Wave 3: builder + skill-identity + config plumbing

- [x] fix · [x] verify · **W3-A.** Move `SkillIdentityRegistryState`,
  `build_skill_identity_registry`, plus the W1-E-deferred
  `ArchiveRuntimeCleanup` (now generalized via two thin traits
  `ArchiveRuntimeMcpState` and `ArchiveRuntimeMobState`) and the
  W1-C-deferred `RuntimePreAdmissionRegistration` (generalized via a
  thin trait `RuntimePreAdmissionRestore`) to
  `meerkat::session_runtime::runtime_state` and `admission`. RPC
  implements the traits and constructs the moved types directly.
  Three regression tests exercise the new shapes
  (`session_runtime_runtime_state.rs`,
  `session_runtime_admission.rs`).

- [x] fix · [x] verify · **W3-B.** `MeerkatSessionRuntime` is now a
  populated struct holding the `service`, `staged_sessions`,
  `staged_capacity_admissions`, `runtime_adapter`, `live_adapter_host`,
  `config_runtime`, `default_llm_client`, `realm_id`, `instance_id`,
  `backend`, and `skill_identity_registry` slots. Accessors
  (`config_runtime`, `realm_id`, `instance_id`, `backend`,
  `default_llm_client`, `live_adapter_host`, `skill_identity_registry`)
  and setters (`set_realm_context`, `set_default_llm_client`,
  `set_live_adapter_host`, `set_skill_identity_registry`,
  `set_skill_identity_registry_for_generation`, `set_config_runtime`)
  live on `MeerkatSessionRuntime`. RPC's `SessionRuntime` keeps its
  duplicate field copies (each an `Arc` clone of the same shared
  state) plus an `inner: Arc<MeerkatSessionRuntime>` field, exposes an
  `inner()` accessor, and updates `set_realm_context` to mirror the
  change into the inner. Phase 4 R1 will collapse the duplicates.
  *Deviation:* the W2-A "deferred load-bearing methods"
  (`precheck_live_open`,
  `recover_live_session_for_realtime_open`,
  `materialize_staged_session_for_realtime_open`,
  `realtime_session_open_config`, `live_open_config_for_session`,
  `propagate_config_to_live_channels`) stay in `meerkat-rpc`. They
  call into RPC-private helpers (`recovery_overrides_from_turn`
  consumes `crate::handlers::turn::TurnOverrides`,
  `recovery_external_tools` builds a `CallbackToolDispatcher`,
  `cleanup_recovered_runtime_if_new` builds an `ArchiveRuntimeCleanup`
  populated with RPC-private MCP/mob hooks), so moving them onto
  `LiveOrchestrator<'a>` would require either (a) duplicating the
  helpers upstream, (b) changing their parameter shape, or (c)
  smuggling an `Arc<dyn …>` per RPC-private dependency through the
  orchestrator. The R11 "live-channel close on identity change" gate
  passes (8/8) without the move, confirming the s71/s72 e2e fix is
  preserved in `meerkat-rpc`. The move is properly tracked as Phase
  4 R1 work.

- [x] fix · [x] verify · **W3-C.** `SessionRuntimeBuilder` now
  implements `new(service, staged_sessions, runtime_adapter)` plus
  `with_live_adapter_host`, `with_live_adapter_host_slot`,
  `with_config_runtime`, `with_config_runtime_slot`,
  `with_default_llm_client`, `with_default_llm_client_slot`,
  `with_realm_id`, `with_realm_id_slot`, `with_instance_id`,
  `with_instance_id_slot`, `with_backend`, `with_backend_slot`,
  `with_skill_identity_registry`,
  `with_skill_identity_registry_slot`,
  `with_skill_identity_context_root_slot`,
  `with_skill_identity_user_root_slot`,
  `with_staged_capacity_admissions`, and `build`. RPC's
  `SessionRuntime::new` and `new_with_config_store` both delegate
  through `SessionRuntimeBuilder::new(...)` to construct the inner
  `MeerkatSessionRuntime`. The CLI / examples /
  `meerkat-rpc/tests/*` paths that go through
  `meerkat_rpc::session_runtime::SessionRuntime::new(...)` continue
  to compile unchanged. Four regression tests in
  `meerkat/tests/session_runtime_builder.rs` exercise empty-builder
  defaults, `set_realm_context` propagation, the
  generation-guard skill registry write path, and `with_realm_id`
  / `with_instance_id` / `with_backend` pre-population.

### Phase 4 — Rewire (sequential, gates on all waves)

- [ ] fix · [ ] verify · **R1.** `meerkat-rpc::session_runtime::SessionRuntime`
  becomes a thin RPC adapter: holds an `Arc<MeerkatSessionRuntime>`,
  delegates every surface-agnostic method, and adds RPC-specific
  helpers (`session_error_to_rpc`, `RpcError`-returning shims for
  `start_turn`/`create_session`, `RpcMobSessionService`,
  comms/skills/mob/MCP wire glue). The file shrinks by an order of
  magnitude. **Closes V4-1 by definition** (the Wave-3 verifier
  flagged that `realm_id` / `instance_id` / `backend` are write-mirrored
  from RPC into the inner `MeerkatSessionRuntime` via `set_realm_context`
  rather than slot-shared at construction; today both sides stay
  consistent because `set_realm_context` is the only writer, but R1
  collapses the duplication entirely so the concern dissolves).
  **Also addresses the W2-A "load-bearing-method deferral":** the W3-B
  agent left `precheck_live_open`, `recover_live_session_for_realtime_open`,
  `materialize_staged_session_for_realtime_open`,
  `realtime_session_open_config`, `live_open_config_for_session`,
  and `propagate_config_to_live_channels` in `meerkat-rpc::SessionRuntime`
  because they consume RPC-private helpers. R1 either inlines the
  helpers, lifts them to `MeerkatSessionRuntime`, or refactors the
  signatures so the methods land on `LiveOrchestrator<'a>` at last.

- [ ] fix · [ ] verify · **R2.** `meerkat-cli/src/main.rs` switches
  from `meerkat_rpc::session_runtime::SessionRuntime` to
  `meerkat::session_runtime::MeerkatSessionRuntime` (or directly to
  `SessionRuntimeBuilder`). The `meerkat-cli` crate drops its
  `meerkat-rpc` dep IF no other RPC types are needed.

- [ ] fix · [ ] verify · **R3.** Update `examples/035-mdm-tux-rs`
  and `examples/*` that currently import
  `meerkat_rpc::session_runtime::SessionRuntime`. They should pull
  from `meerkat::session_runtime` directly. Same for any
  `tests/integration/*` test that constructs a SessionRuntime
  outside the RPC server lifecycle.

- [ ] fix · [ ] verify · **R4.** Update `tests/integration/tests/*`
  that import `SessionRuntime`. Move tests that exercise the
  surface-agnostic methods into `meerkat/tests/session_runtime/`.
  RPC-specific behavioural tests stay in `meerkat-rpc/tests/`.

- [ ] fix · [ ] verify · **R5.** `meerkat-rpc::Cargo.toml`: add
  `meerkat = { workspace = true, features = ["live", …] }` if not
  already there. `meerkat-cli::Cargo.toml`: drop `meerkat-rpc` dep
  if R2 fully cut over. Add a `live` feature passthrough on
  `meerkat-cli` and any other surface that ought to expose live.

### Phase 5 — Meta-level adversarial verification

- [x] fix · [x] verify · **V1.** **Lane-collapse hunt.** For every
  method moved in Waves 2-3, enumerate every typed-concept boundary
  it crosses (provider lane, modality lane, transcript lane,
  display-text lane, control vs lossy channel). Confirm none of the
  moves silently collapsed two lanes into one. Report ≤5
  highest-risk suspicions; do not enumerate exhaustively.

- [x] fix · [x] verify · **V2.** **Identity-loss hunt.** Audit every
  cross-crate boundary the moves introduced. For each, list the
  identity types crossing it (`SessionId`, `LiveChannelId`,
  `RealmId`, `SessionLlmIdentity`, `bound_llm_identity`, `RunId`,
  `OperationId`, …). Confirm no `Option<…>`-flatten or `to_string()`
  conversion lost identity. Report ≤5.

- [x] fix · [x] verify · **V3.** **Unhappy-path hunt.** For each
  moved method that returns `Result<…, …>`, trace every `Err` arm
  through the new crate. Verify cleanup runs (`PendingPromotionCleanup`
  drop, `RuntimePreAdmissionRegistration` drop,
  `cleanup_recovered_runtime_if_new`). Audit `?` propagation that
  bypasses Drop guards. Report ≤5.

- [x] fix · [x] verify · **V4.** **Two-path duplication hunt.**
  After all waves land, for each typed concept owned by
  `MeerkatSessionRuntime` (live identity, staged identity,
  bound channel identity, resolved-from-config identity), confirm
  there is exactly one read site and one write site — no shadow
  copy in `meerkat-rpc::SessionRuntime` that could drift. Report ≤5.

- [x] fix · [x] verify · **V5.** **Surface-symmetry audit.** Walk
  every public method on `MeerkatSessionRuntime`. For each, confirm
  it could legitimately be called from the Rust SDK, REST handler,
  CLI subcommand, and embedded `examples/*` — i.e., it does not
  smuggle an `RpcError`, `RpcResponse`, `RpcId`, or any other RPC
  wire type. Report ≤5 leak sites.

### Phase 6 — CI gate

- [ ] fix · [ ] verify · **G1.** `make ci` green (fmt-check,
  legacy-surface-gate, version-parity, lint, lint-feature-matrix,
  test-all, test-minimal, test-feature-matrix, surface-modularity,
  rmat-audit, audit).

- [ ] fix · [ ] verify · **G2.** `make e2e-fast` and `make e2e-system`
  green.

- [ ] fix · [ ] verify · **G3.** Targeted live-lane (s71+s72) green.

- [ ] fix · [ ] verify · **G4.** Full `make e2e-smoke` green
  (36/36).

- [ ] fix · [ ] verify · **G5.** No `meerkat-rpc` import in
  `meerkat-cli/src/main.rs` (grep gate). No
  `meerkat_rpc::session_runtime` import outside `meerkat-rpc/src/`,
  `meerkat-rpc/tests/`, and `examples/035-*` (the RPC-specific
  example).

## Counts

- **Phase 0 (Foundation):** 4 items (F1, F2, F3, F4)
- **Phase 1 (Wave 1):** 5 items (W1-A through W1-E)
- **Phase 2 (Wave 2):** 5 items (W2-A through W2-E)
- **Phase 3 (Wave 3):** 3 items (W3-A through W3-C)
- **Phase 4 (Rewire):** 5 items (R1 through R5)
- **Phase 5 (Verify):** 5 adversarial passes (V1 through V5)
- **Phase 6 (CI gate):** 5 gates (G1 through G5)
- **Total actionable:** 22 implementation items + 10 verify-pass
  artifacts (`[ ] verify` slots)

## Done definition

All 22 implementation checkboxes ticked AND all corresponding verify
checkboxes ticked AND CI green AND `make e2e-smoke` 36/36 passing
AND no surface other than `meerkat-rpc` imports
`meerkat_rpc::session_runtime`.
