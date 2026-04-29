# Wave (c) — Shell-Code Rebuild Plan

**Branch:** `dogma/wave-a-demolition`. Wave-c opens on the foundation crates (`meerkat-machine-schema`, `meerkat-machine-kernels`, `meerkat-machine-codegen`, `meerkat-core`, `meerkat-contracts`) green. As of this revision, wave-b is landed through B-10 (commits `d3a4b3df8`, `691e9d682`, `d5cccf6bc`, `0e66b2346`, `77554ed80`, `99e155992`); downstream crates are broken.

Wave-c is a retyping pass on consumer code plus a short list of design decisions where the typed foundation collapsed a formerly-implicit seam. No shadow state is reintroduced. Callers that became obsolete because wave-a deleted their semantic owner are *deleted*, not retyped. Six late findings (roster/peer separation, DSL peer retype, composition-dispatcher producer+consumer wiring, OwnerProvided binding extension, SessionStore append-only hardening, persistence v0→v1 migration) fold into the task list rather than living as follow-ups.

This plan integrates seven prior sources, all in `docs/wave-c-prep/`: `wave-c-plan.md` (base draft, `907dd711a`), `persistence-migration.md` (`c970cadcd`), `test-coverage-audit.md` (`f51777501`), `dogma-blind-spots.md` (`5a39c15e1`), `realtime-substrate-audit.md` (`a92e3abf1`), `machine-boundary-stability.md` (`33a0e4d72`), `state-scope-audit.md` (`38ea190e3`). An adversarial review (`wave-c-plan-adversarial-review.md`, `f94da0bcb`) and a post-B-10 re-verification pass surfaced citation drift and allowlist gaps fixed in this revision.

---

## Section 1 — Integrated task list

Tasks are grouped by purpose. Dependencies reference wave-b foundations (V-prefix, B-prefix) and intra-wave-c tasks (C-prefix).

### Core retype spine

**C-1 · meerkat-core consumer retype.** Files: `meerkat-core/src/agent.rs`, `agent/{state,runner,builder}.rs`, `interaction.rs`, `ops_lifecycle.rs`, `session_recovery.rs`, `session.rs`, `service/mod.rs`, `event.rs`, `lib.rs`, `hooks.rs`, `runtime_bootstrap.rs`, `config{,_runtime,_store}.rs`, `auth/token_store.rs`, `lifecycle/run_primitive.rs`; plus `meerkat-contracts/src/wire/runtime.rs` (type relocation only — see below). **Deliverable:** `meerkat-core` compiles; C-SKL (`SkillId` → `SkillKey`) purged; C-TRP retyped to `PeerId` + `TrustedPeerDescriptor` (routing subset); C-CSS dead re-export deleted; C-TM-V3 typed with `from_legacy_value` migration helper. Also adds `ProviderTag::Unknown { bag: StructuredProviderExtension }` on core. `StructuredProviderExtension` currently lives in `meerkat-contracts/src/wire/runtime.rs:218`; C-1 relocates the type into `meerkat-core` and re-exports it from `meerkat-contracts` (adversarial review flaw 5). Without this relocation, core cannot name the bag and unknown provider knobs silently drop (see persistence-migration.md §3.1). **Deps:** V3, V4, V5, V6, V7 green. **Size:** medium (~16 files + one cross-crate type move). **Risk:** core rebuild cascade — start of spine, blocks everything downstream. The `StructuredProviderExtension` move triggers a C-2 schema regen touch but no wire-shape change (re-export preserves path).

**C-2 · meerkat-contracts cleanup.** Files: `lib.rs`, `emit.rs`, `rpc_catalog.rs`, `session_locator.rs`, `wire/{connection,mob,params,realtime,runtime,session,supervisor_bridge,mod}.rs`. **Deliverable:** retired re-exports purged; `wire/mob.rs:152` (`provider_params: Option<Value>`) retyped to `ProviderParamsOverride`; `emit.rs`/`rpc_catalog.rs` drop retired `session/{retire,reset,submission,submissions}` verbs; `SessionLocator.realm_id: Option<String>` (`session_locator.rs:8`) retyped to `Option<RealmId>`; `wire/session.rs` `args: Value` fields audited against dogma-blind-spots.md §7 — tool-call args stay `Box<RawValue>` pass-through as allow-listed, but no other `Value` survives at wire-session boundary. Includes the `StructuredProviderExtension` re-export stub from C-1. **Deps:** V-foundations (B-9 landed), C-1 (type relocation). **Size:** small (~9 files). **Risk:** SDK wire shape changes — C-REGEN (see §6) runs `make regen-schemas` before wave-c completion gate; stale SDK codegen is caught in-wave, not deferred.

**C-3 · meerkat-session persistence migration (v0 → v1).** Files: `meerkat-session/src/persistent.rs`, `ephemeral.rs`, new `meerkat-session/src/persistent/migrations.rs` submodule, new `meerkat-session/tests/persistence_compat.rs`, fixture tree `meerkat-session/tests/fixtures/pre_wave_b/`. **Deliverable:** per-entity schema version bytes land as designed in `persistence-migration.md` §2: bump `SESSION_VERSION = 2`; add `schema_version: u32` to `SessionMetadata` and `stored_input_state_version: u32` to `InputStateSerde`. Opportunistic upgrade-on-read, rewrite as v1 on next save. Fixtures 1-11 from §5 (empty metadata, OpenAI/Anthropic/thinking provider params, slug valid/invalid, hot-swap identity mixed, input-state full/minimal/unknown-provider) live under C-3. `rkat debug migrate-sessions` CLI as stretch. **Deps:** C-1, C-2. **Size:** medium. **Risk:** silent lossy migration on unknown provider knobs — catching assertion is the Anthropic `thinking` fixture (§5 #4), explicitly designed to be the one most likely to be silently dropped. **Note:** fixture #12 (`runtime_session_snapshot_drift.json`) targets `meerkat-runtime/src/store/sqlite.rs:87-101,224-263` (runtime-owned snapshot table), not a session-owned surface — it moves to C-6r (see expanded allowlist).

**C-4 · meerkat-tools + meerkat-skills retype.** Files: `meerkat-tools/src/dispatcher.rs`, `builtin/composite.rs`, `builtin/skills/{browse,load,resources,functions}.rs`, `meerkat-skills/src/{resolve,source/{filesystem,composite,embedded,protocol,git,http}}.rs`. **Deliverable:** C-TOOL-ERR fixed (4 `NotFound` → `AccessDenied` at dispatcher); builtin tools consume `SourceIdentityRegistry::canonical_skill_key`; `SkillRef::Legacy` deleted from sources. **Deps:** V4, V7. **Size:** small-medium (~12 files). **Risk:** skill resolver precedence regression (see C-T below) — tests rebuilt under C-T.

**C-5 · meerkat-comms retype.** Files: `runtime/comms_runtime.rs`, `trust.rs`, `router.rs`, `inbox.rs`. **Deliverable:** `TrustStore` keyed by `PeerId`; `Router::send(dest: PeerId)`; name→id ambiguity is a typed error. **Deps:** V5. **Size:** medium (~5 dense files). **Risk:** `PeerName`-keyed maps leaking outside the crate — catching assertion: `rg 'HashMap<PeerName|BTreeMap<PeerName' --type rust` returns zero hits outside display-only sites.

### Producer/consumer wiring (B-5 handoff — LATE FINDING C)

B-5 (`856500ceb`) delivered the typed `CompositionDispatcher` trait, `CatalogCompositionDispatcher` impl, and `CompositionBinding<E>::{Standalone, Wired(Arc<dyn ...>)}` witness at `meerkat-runtime/src/composition/`. B-10 (`d5cccf6bc`) shipped two live RMAT rules: `CompositionDispatchIsThePath` (byte-level banned-helper scan) and `CompositionRouteSemanticCoverage` (typed route agreement) — both at `xtask/src/rmat_audit.rs:488-614`. These rules are enforced in `rmat-audit --strict`; wave-c must produce code that passes them.

**C-6p · Composition dispatcher producer wiring (mob side).** Files: `meerkat-mob/src/mob_machine.rs`, `meerkat-mob/src/machines/mob_machine.rs` (schema emitter for routed effects), `meerkat-machine-schema/src/catalog/dsl/mob_machine.rs` (generated DSL mirror; regen), `meerkat-mob/src/runtime/actor.rs`, `meerkat-mob/src/runtime/ops_adapter.rs`, `meerkat-mob/src/runtime/event_router.rs`. Codegen path `meerkat-machine-codegen/src/{artifacts.rs:900-935, render.rs:4487}` already exists; C-6p is wiring, not emit-creation. **Deliverable:** every routed effect emitted from the mob actor or DSL flows through `CompositionDispatcher::dispatch`, not through direct peer calls. String `module_path` driver declarations (C-DEAD-DRV, 3 sites) replaced with typed `CompositionDispatcherHandle`. `MobMachineCommand::{EnsureMember, Reconcile, ListMembersMatching}` (at `meerkat-mob/src/mob_machine.rs:46,51,57`, currently `#[allow(dead_code)]` per B-10 carve-out) are either (a) wired through the composition dispatcher or (b) deleted with a commit-message justification (finding I). **Deps:** C-1, V2 (B-5). **Size:** small-medium. **Risk:** dispatcher becoming an "optional fast path" where the old direct call still exists next to the typed path. Catching assertion: `rmat-audit --strict` passes with `CompositionDispatchIsThePath` and `CompositionRouteSemanticCoverage` rules green on the producer side.

**C-6c · Composition dispatcher consumer wiring (meerkat-machine side).** Files: `meerkat-runtime/src/meerkat_machine/mod.rs`, `meerkat-runtime/src/meerkat_machine/dispatch_*.rs`, `meerkat-runtime/src/mob_adapter.rs`, **and `meerkat-runtime/src/handles/**/*.rs`** (adversarial review flaw 2). `grep -c apply_input` across `meerkat-runtime/src/handles/` reports 87 call-sites in 11 files (turn_state.rs 29, external_tool_surface.rs 13, mod.rs 8, interaction_stream.rs 7, peer_interaction.rs 8, session_admission.rs 7, comms_drain.rs 5, mcp_server_lifecycle.rs 5, plus three more) — all of these bypass any future dispatcher if the allowlist excludes them. **Deliverable:** `MeerkatMachine` implements `ConsumerSurface`; `dispatch_*.rs` accept inputs via the dispatcher seam; every `dsl.apply_input(...)` call site inside `meerkat-runtime/src/handles/` routes through `CompositionDispatcher` for routed-effect inputs. Hand-off inputs that are intra-machine (same DSL, no route) stay on the direct path and are explicitly annotated. **Deps:** C-6p. **Size:** large (re-classified from medium after handle audit). **Risk:** duplicate DSL drift between mob composition declarations and the schema catalog — catching assertion: `rmat-audit --strict` `CompositionRouteSemanticCoverage` rule cross-references both. A synthetic-violation injection test confirms the rule flags a real violation (remove a dispatch call from a test fixture, confirm the lint reports, restore).

**C-6o · OwnerProvided binding extension (issue #342).** Files: `meerkat-runtime/src/composition/mod.rs` (where `CompositionBinding<E>` is defined at `:309-315`), plus wherever `MeerkatMachine::with_composition` / `standalone` are constructed. The adversarial review's flaw 3 correctly flagged that `meerkat-machine-codegen/src/composition.rs` does NOT exist — codegen files are `artifacts.rs`, `lib.rs`, `render.rs`, and `RouteBindingSource::OwnerProvided` is already handled at `render.rs:4487`, `artifacts.rs:1085,4914`, `meerkat-machine-schema/src/composition.rs:1093,1714`. Wave-a deleted `composition_dispatch.rs` for cause; B-5 replaced it with `meerkat-runtime/src/composition/`. C-6o does NOT reintroduce `composition_dispatch.rs`; it extends the new typed binding in place. **Deliverable:** `CompositionBinding` gains a third variant `OwnerProvided(Arc<dyn ContextProvider<E>>)` for routes where `session_id` isn't in the producer effect body. Typed context struct, not `Value`. **Deps:** C-6p, C-6c (real dep — C-6c must add the consumer-side constructor seam first; not a parallel sibling). **Size:** small. **Risk:** scope creep — if the typed context struct grows open-ended, stop and defer. Catching assertion: `ContextProvider<E>` has exactly one method and no `Value` anywhere in its signature.

**C-6r · meerkat-runtime retype.** Files: `meerkat-runtime/src/comms_bridge.rs`, `comms_drain.rs`, `meerkat_machine/dispatch_*.rs` (if not already under C-6c), `ops_lifecycle.rs`, `runtime_loop.rs`, `driver.rs`, `meerkat_machine/dsl.rs`, **`meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs`** (schema twin of `PeerEndpoint` at `:3037`, plus `AddDirectPeerEndpoint`/`RemoveDirectPeerEndpoint` inputs at `:427-430, 2714-2740`), and `meerkat-runtime/src/store/sqlite.rs` (the `runtime_session_snapshots` read/write path at `:87-101,224-263`; carries persistence fixture #12 from C-3). **Deliverable:** V3 single-site `RuntimeTurnMetadata::for_input`; V5 `PeerId` threaded; TrustedPeerSpec retyped; the DSL `PeerEndpoint { name: String, peer_id: String, address: String }` at `meerkat-runtime/src/meerkat_machine/dsl.rs:1721-1757` AND its structural twin at `meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs:3037` retyped to `PeerEndpoint { name: PeerName, peer_id: PeerId, address: PeerAddress }` in lockstep (the dsl.rs:1715-1720 in-line comment mandates structural equivalence; retyping one without the other breaks the schema validator's structural-equivalence check — adversarial review flaw 4). `From<TrustedPeerSpec> for PeerEndpoint` stops erasing typing at the catalog seam (LATE FINDING B). **Deps:** C-1, C-5, V2, V3, C-6c. **Size:** large (≥10 files; core semantic seams). **Risk:** choke point for the fan-out tier below; stay on consolidation branch. Catching assertion: `xtask::machines::check_dsl_parity` run at end of C-6r confirms both `PeerEndpoint` copies share typed fields; `verify-schema-freshness` passes.

### Mob + downstream consumers

**C-7 · meerkat-mob retype.** Files: `meerkat-mob/src/machines/mob_machine.rs`, `runtime/{actor,handle,tools,provisioner,provision_guard,supervisor_bridge,ops_adapter,actor_turn_executor,disposal,builder,edge_locks,event_router}.rs`, `roster.rs`, `event.rs`, `build.rs`, `profile.rs`, `tests.rs`, `tests/{contracts,phase1_red_ok}.rs`, `tests/member_{binding_orthogonality,session_bindings}.rs`. **Deliverable:** the `MemberSessionBindingChanged { epoch, agent_identity, old_session_id, new_session_id }` single-emit-point already landed at `machines/mob_machine.rs:1049, 1663, 1681, 1698` with DSL guards (`no_prior_session_binding`, `prior_session_binding_present`, `old_session_id_matches_current`) that prevent `(None, None)` at authority time by construction (adversarial review flaw 11). C-7's retype scope is therefore: **consumer-side migration** — every caller still pattern-matching on the retired `Set`/`Rotated`/`Released` trio is retyped to the collapsed variant. C-TRK-B-CMDS deletion; C-AGENT-RT-ID tightening (`MobMemberSnapshot.agent_runtime_id` / `.fence_token` drop from `pub` to `pub(crate)`; external consumers read `MobMemberView`); TrustedPeerSpec retyped; orphan tests rewritten/deleted. **Deps:** C-1, C-5, C-6r. **Size:** large (~20 files). **Risk:** consumer retype misses a callsite. Catching assertion: `grep "MemberSessionBindingSet\|MemberSessionBindingRotated\|MemberSessionBindingReleased" meerkat-*/src/` returns zero hits.

**C-7r · Roster external-peer separation (LATE FINDING A — verify before starting).** Files: `meerkat-mob/src/roster.rs`, `meerkat-mob/src/event.rs`, `meerkat-mob/src/runtime/actor.rs`. **Status:** verification against HEAD (adversarial review flaw 10) confirms `From<PeerName> for AgentIdentity` has **already been deleted** (no impl remains workspace-wide; only `impl From<PeerName> for String` at `meerkat-core/src/comms.rs:198` survives). `meerkat-mob/src/roster.rs:86` currently types `wired_to: BTreeSet<AgentIdentity>` and a separate `external_peer_specs: BTreeMap<AgentIdentity, TrustedPeerSpec>` at `:102`. PR #340 (`RecomputeMobPeerOverlayDriver` typed translation) is already on the boundary. **Revised deliverable:** audit `meerkat-mob/src/runtime/actor.rs` handlers for `ExternalPeerWired`/`Unwired` to confirm no residual coercion path re-introduces `AgentIdentity::from(peer_name_string)` style entries; if clean, C-7r closes as a zero-line task with a commit noting the verification. If any residual coercion is found, retype in place. **Deps:** C-7. **Size:** small (verification-first; potentially zero-line). **Risk:** stale finding — verify-and-close rather than retype. Catching assertion: `grep -rn "AgentIdentity::from" meerkat-mob/src/` returns only constructor-from-string at test/fixture sites.

**C-8 · meerkat-mob-mcp.** Files: `lib.rs`, `public_mcp.rs`, `agent_tools.rs`. **Deliverable:** consumes `MobMemberView`; TrustedPeerSpec retyped; `realtime_status_from_mob_status` dead-code projection deleted (C-R2). **Deps:** C-7, V5. **Size:** small.

### Realtime substrate (from `realtime-substrate-audit.md`)

Ten subtasks from the audit, split by deliverable to avoid a single >15k-LOC task (adversarial review Section 4 concern; also lesson from B-4 scope-wall mid-execution).

**C-9a · RPC typed verbs + realm/binding retype.** Files: `meerkat-rpc/src/router.rs`, `session_runtime.rs`, `main.rs`, `handlers/{session,mob,runtime,auth}.rs`. **Deliverable:** retired RPC verbs deleted (`session/{retire,reset,submission,submissions}`); `handlers/runtime.rs` consumes typed `SessionAcceptInputParams`/`WireInputState`; `.realm_id`/`.binding_id` retyped workspace-wide inside meerkat-rpc; SkillId purged. **Deps:** C-1, C-2, C-6r. **Size:** medium.

**C-9b · Realtime WS rotation correctness (C-R1/R7/R8/R9).** Files: `meerkat-rpc/src/realtime_ws.rs`, `meerkat/src/realtime.rs`, `meerkat-rpc/src/handlers/realtime.rs`. **Deliverable:** (R1) delete panicking `RealtimeChannel::mob_member` deprecation stub; (R7) `session.close()` timeout in product-session actor at `realtime_ws.rs:2885`; (R8) on broadcast `Lagged` at `realtime_ws.rs:1965`, re-resolve the binding map via `mob_handle.current_realtime_binding(identity)` (closes §3.3 socket-pinned-to-retired-session gap); (R9) invariant — every `handle.abort()` on a realtime task is paired with `detach_live`, enforced by grep audit + optional AST lint. Realtime WS reads the collapsed `MemberSessionBindingChanged` (consumer-side of C-7). **Deps:** C-7, C-9a. **Size:** medium.

**C-9c · Status projection collapse (C-R3/R4).** Files: `meerkat-runtime/src/meerkat_machine_types.rs`, `meerkat-rpc/src/handlers/realtime.rs`, `meerkat-mob-mcp/src/public_mcp.rs`. **Deliverable:** (R3) `projection_to_channel_status` / `realtime_status_from_runtime` collapsed into a single canonical `impl From<RealtimeAttachmentStatus> for RealtimeChannelStatus` in `meerkat-runtime/src/meerkat_machine_types.rs` (or a `meerkat-contracts` helper); both RPC and MCP delegate. (R4) typed `attempt_count` / `next_retry_at` in `RealtimeAttachmentStatus` — reconnect retry machine flows into DSL input; status queries see real retry state, not hard-coded `0/1`. **Deps:** C-6r. **Size:** small-medium.

**C-9d · Typed realtime results + error codes (C-R5/R6).** Files: `meerkat-contracts/src/wire/realtime.rs`, `meerkat-rpc/src/handlers/realtime.rs`, `meerkat-rpc/src/realtime_ws.rs`. **Deliverable:** (R5) typed `RealtimeActionResult` for interrupt/close on `RealtimeProductSessionCommand`; kill the `preemptive_interrupt_can_be_ignored` message-string match. (R6) expand `RealtimeErrorCode`; rewrite `realtime_client_error_frame`. Auth/ContentFiltered/ModelNotFound each get a typed code. **Deps:** C-2. **Size:** small.

**Deps across C-9 subtasks:** C-9a → C-9b, C-9c and C-9d parallel with C-9a. `meerkat/src/realtime.rs` is owned by C-9b; C-13 (facade) coordinates via serial ordering on that file (see Section 3).

**C-R10 — Realtime schema regen** is covered by C-REGEN (§6 completion step), no longer listed as out-of-scope.

### Surface crates

**C-10 · meerkat-rest.** Files: `meerkat-rest/src/lib.rs`, `schedule_host.rs`, `auth_endpoints.rs`. **Deliverable:** C-REST-READMUT fix — verified against HEAD: `ensure_runtime_session_registered` lives at `meerkat-rest/src/schedule_host.rs:72` with two call sites at `:645, :691` (NOT `lib.rs:1280,1915,1986,2663,3495` as earlier drafts cited; those lines are stale — adversarial review flaw 6). Audit those two call sites: both are in a schedule-host dispatch path; confirm neither is a read-only query before deletion. If both are write-adjacent (legitimate registration), the task reduces to "no change; helper is correctly placed." If either is read-only, delete the ensure-call from that arm. Remaining scope: four `json!({…})` sites become typed `Json(wire)`; string-form `ConnectionRef` rejected at 400; `.realm_id`/`.binding_id` retyped. **Deps:** C-2, C-6r. **Size:** medium (mostly the json!→typed migration; REST READMUT may be near-zero after verification).

**C-11 · meerkat-mcp-server.** Files: `lib.rs`, `main.rs`, `runtime_ingress.rs`, `schedule_host.rs`. **Deliverable:** `.realm_id` retyped; string-form `ConnectionRef` ingress rejected; SkillId purged. **Deps:** C-2, C-1. **Size:** small.

**C-12 · meerkat-cli.** Files: `meerkat-cli/src/main.rs`, `mcp.rs`, new `cli_parse.rs`. **Deliverable:** sole `parse_connection_ref_user_input(&str) -> Result<ConnectionRef, CliError>` lives in `cli_parse.rs`. The `main.rs:3798` site parses a `TokenKey` via `split_once(':')`; the `mcp.rs:193` site splits HTTP headers via `splitn(2, ':')` — these are NOT both ConnectionRef parsers. Only the ConnectionRef-parsing call site is migrated; HTTP-header split stays put (adversarial review flaw 7). `.realm_id`/`.binding_id` retyped across the crate — verified count: `grep -c "\.realm_id\|\.binding_id" meerkat-cli/src/main.rs` reports 62 sites (NOT 122 or 278; earlier drafts conflated realm+binding+profile counting — adversarial review flaw 8). C-CLI-DISCRIM fixed. SkillId purged. **Deps:** C-2. **Size:** small-medium (sized on verified 62-site count, not the earlier 278-site estimate). **Risk:** ad-hoc parser regrowth; catching assertion: `meerkat-cli/tests/connection_ref_single_parser.rs` asserts exactly one `ConnectionRef`-parsing call site — scoped by function name and wrapping module, not by `split_once` byte-pattern.

**C-13 · meerkat facade.** Files: `meerkat/src/factory.rs`, `service_factory.rs`, `surface.rs`, `surface/{runtime_backed,runtime_schedule_host,schedule_host}.rs`, `prompt_assembly.rs`, `lib.rs`. **Note:** `meerkat/src/realtime.rs` is owned by C-9b (not C-13); if C-13 needs incidental edits to that file, it rebases on C-9b. **Deliverable:** V3 single-site `RuntimeTurnMetadata`; `None =>` fallback arms in `factory.rs` become typed "ambient credential selection refused" errors (dogma #8 stance); SkillId purged. Re-verify the `factory.rs` fallback-arm line numbers against HEAD at task-start (earlier drafts cited `:1241,1684,1819,2306,2884,2900`; these should be confirmed or updated). **Deps:** C-1, C-2, C-6r, C-9b (for `meerkat/src/realtime.rs` ordering). **Size:** medium-large.

**C-14 · provider crates.** Files: `meerkat-anthropic/src/runtime/mod.rs`, `meerkat-openai/src/runtime/mod.rs`, `meerkat-openai/src/realtime_attachment.rs` (C-R9), `meerkat-gemini/src/runtime/mod.rs`, `meerkat-llm-core/src/{adapter,provider_runtime/registry,types}.rs`, `meerkat-auth-core/src/{auth_store/{file,refresh},resolver}.rs`. **Deliverable:** `.realm_id`/`.binding_id` retyped; typed `ProviderParamsOverride` at the provider boundary. **Deps:** C-2. **Size:** small-medium.

**C-16 · web / WASM.** Files: `meerkat-web-runtime/src/lib.rs`, `sdks/web/src/*.ts` (Rust-side bindings only). **Deliverable:** retyped V5/V6; `allow_web_build` string-form bypass deleted. **Deps:** C-13. **Size:** small.

### Hardening

**C-H1 · SessionStore append-only hardening (F1/F7 from state-scope-audit.md).** Files: `meerkat-core/src/session_store.rs`, `meerkat-core/src/session.rs`, `meerkat-session/src/**`, **`meerkat-store/src/**`** (SqliteSessionStore, JsonlStore, MemoryStore implementations — the append-only constraint rewrites their `save()` semantics), and any adapters in `meerkat-rpc/`, `meerkat-rest/`, `meerkat/src/` that call `SessionStore::save` directly. **Deliverable:** Option B from §3 of the state-scope audit — formalize the doc ("snapshot = projection"), route all persistence through `EventStore`, make `SessionStore` a test-only convenience via an `AppendOnlySessionStore` alias trait that guards `save()` as `append_or_extend_only`. The `Session.messages: Arc<Vec<Message>>` gets an `AppendOnlyMessages` newtype exposing only `.push()`/`.extend()`; `fork_at` becomes an explicit fork on a **new** `SessionId` (adversarial review R-B: the current `fork_at` at `meerkat-core/src/session.rs:798` returns a Session with the same SessionId; C-H1 must rotate the id in the method body, otherwise F7 is not closed). **Deps:** C-1, C-3. **Size:** medium. **Risk:** surface-level callers relying on `save()` replace semantics, or on `fork_at` preserving `SessionId`. Catching assertion: new test `meerkat-core/tests/fork_at_rotates_session_id.rs` confirms `fork_at` returns a distinct `SessionId`.

**C-H2 · Collapse `comms_drain_slots` into `RuntimeSessionEntry` (F5).** Files: `meerkat-runtime/src/meerkat_machine/mod.rs` (around line 417). **Deliverable:** `CommsDrainSlot` moves into `RuntimeSessionEntry`; single HashMap insertion for "session exists"; eliminates the two-maps-desync bug class. **Deps:** C-6r. **Size:** small.

**C-H3 · Inbox TOCTOU fix (flagged in `dogma-blind-spots.md` §7 commentary).** Files: `meerkat-comms/src/inbox.rs` (the `483 vs 505` window). **Deliverable:** tightened single-pass check+apply. **Deps:** C-5. **Size:** small. **Risk:** file a follow-up GitHub issue citing the specific line range so the fix is externally reviewable.

**C-F2 · Trust-edge mutations → formal seam-closure obligation pair (F2 from state-scope-audit.md).** Files: `meerkat-runtime/src/meerkat_machine/dsl.rs` (supervisor_bound_* inputs, see in-line DSL comment at `:114-134`), `meerkat-comms/src/router.rs` (`TrustedPeers`). **Deliverable:** per state-scope-audit §3 row F2 — either (a) move the trust-edge mutation into AuthMachine authority and consume from MeerkatMachine via projection, or (b) formalize the step-lock as a generated obligation pair so the shell cannot forget to mirror. Preferred: pull trust-edge mutations into AuthMachine authority. **Deps:** C-5, C-6r. **Size:** small-medium. **Risk:** Auth/Meerkat boundary migration scope creep. Catching assertion: new generated obligation pair surfaced in seam-inventory (B-10 infrastructure).

**C-F3 · Mob destroy → session ingress detach handoff (F3 from state-scope-audit.md).** Files: `meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs:112` (peer_ingress_mob_id), `meerkat-mob/src/runtime/actor.rs` (destroy path). **Deliverable:** add `MobDestroyingSessionIngress` handoff obligation so mob-destroy → session-detach is compile-time paired — document the contract explicitly: MeerkatMachine tolerates dangling MobId iff mob destruction propagates a `DetachIngress` input before destroying the mob. **Deps:** C-6r, C-7. **Size:** small. **Risk:** missed destroy path still orphans ingress references. Catching assertion: xtask obligation-pair check flags an unpaired destroy.

**C-WAR · Wave-a residue fixes surfaced by `rmat-audit --strict`.** B-10 (`d5cccf6bc`) flagged these pre-existing failures (not introduced by wave-b, but now visible because xtask compiles). Required for `rmat-audit --strict` to stay green as part of the CI gate. Files and deliverable:

- **WAR-1 · `NoDeadAuthorityWiring`** at `meerkat-mob/src/runtime/actor.rs:986` (`machine_active_run_count` currently carries `#[allow(dead_code)]`; used at `:1002, :2120`). Either wire the read seam through a typed projection or remove the allow and the method together. Deps: C-7. Size: tiny.
- **WAR-2 · `NoGuardedApply`** somewhere in `meerkat-mob/src/runtime/actor.rs` (B-10 flagged a conditional `apply` that should be an unconditional submit). Verify location via `rmat-audit --rule NoGuardedApply` at task-start; convert to unconditional submit. Deps: C-7. Size: tiny.
- **WAR-3 · `RuntimeExternalEventProjectionAlignment`** in `meerkat-runtime/src/runtime_loop.rs` (block-renderables projection gap; multiple flag sites listed in `xtask/src/rmat_audit.rs:1397-1456`). Deliverable: align the external-event projection with the DSL's authoritative block-renderable shape. Deps: C-6r. Size: small.

**Deps (C-WAR as a whole):** C-6r, C-7. **Size:** small (three targeted fixes). **Risk:** any WAR-* item expands mid-execution; if so, escalate rather than absorbing silently.

**C-REGEN · SDK schema + generated-types regeneration.** Files: `artifacts/schemas/**`, `sdks/python/meerkat/generated/**`, `sdks/typescript/src/generated/**`. **Deliverable:** run `make regen-schemas` as a mechanical pass after all wire-type-touching tasks (C-2, C-6r, C-9a/b/c/d, C-14) land. Commit regenerated artifacts. This is intentionally in-wave-c because `verify-schema-freshness` is part of the §6 completion gate — deferring regen to wave-d contradicts the completion criterion (adversarial review flaw 12). Wave-d still owns SDK package version bumps, release infrastructure, and doc regeneration from the typed catalog; wave-c only runs the mechanical regen so CI goes green. **Deps:** all tasks that mutate `meerkat-contracts/src/wire/**` or exported wire types. **Size:** small (mechanical). **Risk:** generated diff is larger than expected; review the diff shape, not the line count.

### Tests (the "blocker" list)

**C-T · Test rebuild sweep (covers the 16-item list in `test-coverage-audit.md` §6).** Files: `meerkat-mob/tests/`, `meerkat-runtime/tests/`, `meerkat-comms/tests/`, `meerkat-skills/tests/` (new), `meerkat-tools/tests/` (new), `meerkat-core/tests/`. **Deliverable:** rebuild all 16 items from test-coverage-audit.md §6, split into blocker items 1-9 and concerning items 10-16. Each of the 6 "Lost and blocker" entries from test-coverage-audit §5.3 gets one new test, named below; each of the 10 non-blocker "concerning" behaviors gets one new test or a concrete justification for deletion. Also adds the dispatcher-behavioral test R-C from the adversarial review.

Blocker tests (6 from §5.3 "Lost and blocker", mapped to items 1-9 of §6 rebuild list):
- `meerkat-comms/tests/trust_reconcile_concurrency.rs` — `concurrent_reconciles_are_serialized_and_stale_short_circuits` (§6 #1, #2)
- `meerkat-comms/tests/trust_reconcile_add_failure.rs` — typed error + applied-view untouched (§6 #3)
- `meerkat-mob/tests/respawn_overlay_rotation.rs` — wire M1+M2, respawn M1, prior-session overlay cleared (§6 #4)
- `meerkat-mob/tests/release_binding_overlay_sweep.rs` — release-binding removes member from overlay projection (§6 #5)
- `meerkat-mob/tests/shadow_mode_parity.rs` — DSL projection == actor-restore wiring set (§6 #6)
- `meerkat-mob/tests/mob_member_restore_failure_terminal.rs` — restore-failure → Broken classification (§6 #7)
- `meerkat-runtime/tests/turn_boundary_denial_blocks_effects.rs` — authority denial blocks side-effects (§6 #9)

Concerning tests (covering §6 #10-16):
- `meerkat-runtime/tests/composition_driver_contract.rs` (§6 #10; also the R-C dispatcher-behavioral test — mob actor emitting `MemberSessionBindingChanged` triggers a real `MeerkatMachine::peer_projection` input)
- `meerkat/tests/schedule_host_typed_mapping.rs` (§6 #11; shared surface test for CLI/REST/RPC)
- `meerkat-skills/tests/stdio_protocol.rs` (§6 #12), `filesystem_source.rs` (§6 #13), `resolver_precedence.rs` (§6 #14)
- `meerkat-tools/tests/browse_load_surface.rs` (§6 #15)
- `meerkat-comms/tests/peer_request_promotion.rs` (§6 #16)
- `meerkat-core/tests/agent_run_completed_hook_failure.rs` (§6 #8)

**Deps:** all prior C-* tasks. **Size:** medium (~40 targeted tests including edge cases per blocker). **Risk:** writing DSL-level tests that reference the old authority enums — catching assertion: each blocker-list test is either a `meerkat-machine-schema` DSL reducer test or an actor-path end-to-end test; no hand-written authority reducers.

**C-15 · test-harness sweep.** Files: `meerkat-*/tests/**`, `tests/integration/**`, `examples/**`. **Deliverable:** obsolete tests deleted (wire/unwire, Bind/Rotate/Release); live tests retyped; examples compile; no new `#[allow(dead_code)]`. **Deps:** all prior. **Size:** medium.

### Deferred to post-0.6 (NOT wave-c)

- Issue **#341** (fault-explicit state machines) — the 11 dogma blind-spot classes; `ephemeral.rs:2548-2572` cancellation races are the highest-leverage seam but wave-c does not close them.
- Issue **#343** (Signal-route typed dispatcher — parallel `CompositionSignalDispatcher`) — wave-c keeps the single-dispatcher shape.

---

## Section 1.5 — Tripwire tests (TDD scaffolding, pre-wave-c)

Before any wave-c worker starts, a small set of tests lands that explicitly fail today and must pass by the end of wave-c. Each tripwire:

- exists as a committed test file **before any C-* task begins**
- fails in a specific, diagnosable way today (compile-fails or asserts an invariant the current code violates)
- has a named wave-c task whose completion is what flips it green
- catches the class of regression where a task *appears* to complete but the tripwire surfaces the fake

If wave-c ends green on everything else but these are still red, something has been done wrong. If a worker claims a task done and its tripwire is still red, the task isn't done.

**Tripwire list:**

1. **`meerkat-core/tests/fork_at_rotates_session_id.rs`** — asserts `Session::fork_at(...)` returns a distinct `SessionId`. **Baseline revised at c.0**: `fork_at` at `meerkat-core/src/session.rs:798` ALREADY calls `SessionId::new()` (line 803), so this assertion passes today — the plan's original "fails today" claim was stale. The test stays as a regression guard against a future change that reverts the rotation. F7 closure is really about the typed append-only witness on `Session.messages` and `save()` replace semantics (see Section 1.5 #9 + completion criterion below), not SessionId rotation. Flipped green: passes today; C-H1 is the task that must not regress it.

2. **`meerkat-session/tests/persistence_compat.rs` (12 fixtures)** — v0 fixtures 1-12 from `persistence-migration.md` §5 loaded and asserted for lossless typed round-trip. Fixture #4 (Anthropic `thinking: {type:"enabled", budget_tokens:32000}`) is the canary — the case most likely to be silently dropped by the typed retype. Fails today (no v1 migration exists). Flipped green by **C-3** (fixtures 1-11) + **C-6r** (fixture #12, runtime-side snapshot table).

3. **`meerkat-machine-schema/tests/peer_endpoint_structural_equivalence.rs`** — asserts both `PeerEndpoint` copies (runtime DSL at `meerkat-runtime/src/meerkat_machine/dsl.rs:1721` and schema catalog at `meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs:3037`) carry typed fields (`PeerId`, `PeerAddress`, `PeerName`), not `String`. Fails today (both are `String`). Flipped green by **C-6r**.

4. **`meerkat-cli/tests/connection_ref_single_parser.rs`** — asserts exactly one call site parsing a string into `ConnectionRef` across `meerkat-cli/src/`, scoped by function name (`parse_connection_ref_user_input`), not a byte-pattern match on `split_once(':')` (HTTP-header splits and `TokenKey` parses are separate and must not be flagged). Fails today (multiple ad-hoc sites). Flipped green by **C-12**.

5. **`meerkat-runtime/tests/handles_dispatch_through_dispatcher.rs`** — extension of B-5's grep canary at `meerkat-runtime/tests/composition_dispatch_is_the_path.rs`: asserts every routed-effect-consuming `apply_input` call in `meerkat-runtime/src/handles/**/*.rs` traverses `CompositionDispatcher`. 87 call sites today bypass the dispatcher (per C-6c audit). Flipped green by **C-6c**. This is the tripwire that catches "dispatcher wiring went in but handles weren't touched."

6. **`meerkat-mob/tests/dispatcher_rule_synthetic_injection.rs`** — negative-case test for B-10's `CompositionDispatchIsThePath` rule. Temporarily removes a known `dispatcher.dispatch(...)` call from a test fixture, runs `rmat-audit --rule CompositionDispatchIsThePath`, asserts the rule reports a violation, restores. Proves the rule isn't silently broken — guards against the dispatcher-rule becoming vacuous (false-green). Runs as part of c.1 exit gate.

7. **Blocker-test stubs** (7 files from C-T) — `meerkat-comms/tests/trust_reconcile_concurrency.rs`, `trust_reconcile_add_failure.rs`, `meerkat-mob/tests/respawn_overlay_rotation.rs`, `release_binding_overlay_sweep.rs`, `shadow_mode_parity.rs`, `mob_member_restore_failure_terminal.rs`, `meerkat-runtime/tests/turn_boundary_denial_blocks_effects.rs`. Created as `#[ignore]`-annotated tests that **compile** once the expected API surface exists and **assert** the invariant in test-body form. Fail to compile if the expected API never appears. Un-ignored as each blocker's source-side task lands. Catches "blocker tests bolted on at the end with weak assertions."

8. **`meerkat-machine-schema/tests/named_type_binding_covers_catalog.rs`** — asserts `MachineSchema::validate()` rejects any schema that references a `NamedTypeId` without a matching binding. Fails today for a test fixture with unbound named type. (B-4 landed the validator; tripwire exercises it.) Runs as a cheap backstop that C-6p/C-6c don't accidentally bypass the validator.

9. **`meerkat-session/tests/session_store_append_only_trybuild.rs`** — `trybuild` compile-fail test attempting to call a method on `SessionStore` that would shrink a session's message history. Must compile-fail after **C-H1** lands (type-level append-only). Today it compiles because `save()` allows replacement. This is the compile-time form of F1 closure.

**c.0 — Tripwire scaffolding phase.** Before any C-* task starts, one commit lands all 9 tripwires as failing tests (in the predicted ways). Commit titled `test(wave-c c.0): tripwire scaffolding — TDD backstops for wave-c deliverables`. Each test carries a module-level `//!` doc-comment naming the flipping task (e.g., `//! Flipped green by C-H1`). If any tripwire unexpectedly passes at c.0 commit time, that's a signal the baseline assumption is wrong and the plan needs updating before workers start.

**Ongoing enforcement.** Each wave-c worker's completion checklist includes "confirm the tripwires my task was supposed to flip are now green." This is in addition to the task's own specified catching assertions (Section 5).

---

## Section 2 — Dependency graph

B-10 (`d5cccf6bc`, commits through `99e155992`) shipped the `CompositionDispatchIsThePath` rule (byte-level banned-helper scan) and `CompositionRouteSemanticCoverage` rule (typed route agreement) in `xtask/src/rmat_audit.rs`. Both rules are live in `rmat-audit --strict`. Wave-c's C-6c must produce code that passes these rules; the rules are active backstops, not aspirational. Citation: commits `d5cccf6bc`, `691e9d682`, `77554ed80`, `99e155992`.

```
    [V1..V8 wave-b foundations green + B-10 strict rules live]
                 |
        +--------+--------+
        |                 |
      C-1 core         C-2 contracts
       (relocates            (re-exports
        StructuredProviderExtension)
        |   \ \__________   \
        |    \           \   \
        v     v           v   v
      C-5 comms       C-4 tools/skills      C-14 providers (early)
        |
        v
      C-3 session (legacy v0 fixtures 1-11; #12 moves to C-6r)
        |
        v
      C-6p composition-dispatcher producer (mob DSL + actor + schema mirror)
        |
        v
      C-6c composition-dispatcher consumer (meerkat + handles/*)
        |   \
        |    +--> C-6o OwnerProvided binding (in meerkat-runtime/src/composition/)
        v
      C-6r meerkat-runtime retype (CHOKE POINT) — PeerEndpoint twin + fixture #12
        |
        +--> C-H2 drain-slot collapse
        +--> C-F3 mob-destroy → ingress-detach obligation
        +--> WAR-3 runtime_loop projection alignment
        v
      C-7 meerkat-mob (consumer retype of already-collapsed effect)
        |   \
        |    +--> C-7r roster verify-and-close (likely zero-line)
        |    +--> WAR-1 machine_active_run_count
        |    +--> WAR-2 guarded-apply in actor.rs
        |    +--> C-F2 trust-edge mutation → AuthMachine authority
        +---------> C-8 mob-mcp
        v
    [fan-out parallel: C-9a rpc typed verbs | C-10 rest | C-11 mcp-server | C-12 cli | C-13 facade]
                     \
                      +--> C-9b realtime WS rotation (owns meerkat/src/realtime.rs)
                      +--> C-9c status projection collapse
                      +--> C-9d typed realtime results + error codes
                                                                              |
                                                                              v
                                                                        C-16 web
                                                                              |
                +------- C-H1 SessionStore append-only ----------+            |
                |        C-H3 inbox TOCTOU                       |            |
                v                                                v            v
                                                              C-T blocker-list tests
                                                              C-15 test sweep
                                                              C-REGEN schema + SDK types
```

**Serial spine:** `foundations → C-1 → C-6p → C-6c → C-6r → C-7 → C-9a`. Everything else parallelizes off the spine.

**Critical path for enforcement.** B-10's `CompositionDispatchIsThePath` and `CompositionRouteSemanticCoverage` rules are live in `rmat-audit --strict`; any direct-apply callsite outside the dispatcher will fail strict mode. Gate after C-6c: run `rmat-audit --strict` and confirm clean (for the positive case) and a synthetic injection (remove a `dispatch` call from a test fixture, confirm the lint reports, restore) for the negative case.

**Fan-out after C-7:** C-8, C-9a/b/c/d, C-10, C-11, C-12, C-13, C-14, C-16 partition by crate. `meerkat/src/realtime.rs` is owned by C-9b; C-13 (facade) serializes on that file.

---

## Section 3 — Parallelism / worktree strategy

Base branch stays `dogma/wave-a-demolition`.

**Spine tasks** (C-1, C-6p/c/r, C-7, C-9a) run on the consolidation branch directly. Cross-crate seams; only one author at a time.

**Fan-out tasks** (C-8, C-9b/c/d, C-10, C-11, C-12, C-13, C-14, C-16, C-T) run in per-task worktrees that rebase against the spine's tip when each serial task lands. Worktree isolation prevents concurrent edits to `Cargo.lock`, build caches, and derived `.rkat/` files. **Overlap to watch:** `meerkat/src/realtime.rs` (C-9b vs C-13) — C-13 rebases on C-9b; C-9 and C-13 no longer share edit ownership of that file.

**Commit hygiene.** `git commit -o <paths>` continues. `--no-verify` gets phased out as the tree regains compilation — not earlier; a half-compiling tree fails `cargo fmt --check` and blocks progress. Flip hooks back on at the merge-to-main step. One crate per commit in the fan-out tier; spine commits split per cluster (e.g., "C-1: SkillId purge", "C-1: TrustedPeerSpec retype").

**Named conflict risks.**

1. `Cargo.lock` — only spine touches `Cargo.toml`; fan-out is code-only.
2. `meerkat-contracts/src/lib.rs` re-exports — C-2 owns the final list; downstream crates only consume.
3. `meerkat-cli/src/main.rs` — 62 `.realm_id`/`.binding_id` field-access sites (verified against HEAD; earlier drafts cited 278 after conflating realm+binding+profile counting). Keep C-12 single-agent; no line-range splitting.
4. `meerkat-mob/src/runtime/tests.rs` + `meerkat-mob/tests/member_session_bindings.rs` — C-7 deletes orphans inline with the effect collapse; C-15 only retypes survivors.

---

## Section 4 — Phases within wave-c

**c.0 — Tripwire scaffolding.** Section 1.5's 9 tripwires committed as failing tests. Exit gate: all tripwires compile; each fails in the predicted way (no unexpected passes); commit lands with per-test documentation of the flipping task.

**c.1 — Enabling wiring.** C-1 core + C-2 contracts + C-6p + C-6c. Unlocks every downstream consumer and makes B-10's live rules enforce on real code. Exit gate: `rmat-audit --strict` green; tripwire #6 (synthetic injection) confirms the backstop; tripwire #5 flips green as C-6c lands handle-layer wiring.

**c.2 — Consumer rebuilds (parallel).** C-3 session (parallel with C-4 tools/skills, C-5 comms, C-14 providers) once C-1+C-2 done. Exit gate: each of the 5 crates `cargo check` clean in isolation.

**c.3 — Runtime + mob.** C-6r choke point on spine; then C-6o (OwnerProvided), C-H2 (drain-slot collapse), C-F3 (mob-destroy ingress-detach obligation), WAR-3 (runtime_loop projection alignment), C-7 mob retype, C-7r (roster verify-and-close), WAR-1/WAR-2 (dead-code / guarded-apply fixes), C-F2 (trust-edge mutation migration), C-8 (mob-mcp). Exit gate: `cargo check -p meerkat-mob -p meerkat-mob-mcp` clean; `rmat-audit --strict` clean; composition dispatcher is *the* path.

**c.4 — Surface layer (parallel fan-out).** C-9a rpc typed verbs, C-9b realtime rotation, C-9c status projection collapse, C-9d typed realtime results, C-10 rest, C-11 mcp-server, C-12 cli, C-13 facade (serializes on `meerkat/src/realtime.rs` with C-9b), C-16 web. Exit gate: `cargo check --workspace` clean.

**c.5 — Hardening, regen, and test rebuild.** C-H1 SessionStore append-only (including `fork_at` SessionId rotation), C-H3 inbox TOCTOU, C-T blocker-list tests, C-15 test sweep, C-REGEN schema + SDK regeneration. Exit gate: §6 completion criteria satisfied.

---

## Section 5 — Risk register

Integrated from all seven sources.

1. **`SessionLocator.realm_id: String` vs `ConnectionRef.realm: RealmId` diverge silently.** C-2 retypes `SessionLocator.realm_id` to `Option<RealmId>`. Catching assertion: `meerkat-contracts/tests/locator_realm_typed.rs` round-trips via `RealmId::parse`.

2. **V3 legacy-row deserialize silently drops unknown provider knobs.** The Anthropic `thinking: {type:"enabled", budget_tokens:32000}` case is the one most at risk (persistence-migration.md §5 fixture #4). Catching assertion: `meerkat-session/tests/persistence_compat.rs` loads the thinking fixture and asserts the typed bag round-trips losslessly, not that it silently drops. Depends on C-1 adding `ProviderTag::Unknown { bag }` on core.

3. **C-MEMBER-BIND consumer retype misses a callsite** (rescoped — the effect collapse landed pre-revision; see C-7). Any remaining consumer pattern-matching on the retired `Set`/`Rotated`/`Released` trio compiles against stale shapes. The `(None, None)` case is already rejected at DSL authority time by construction (`no_prior_session_binding`, `prior_session_binding_present`, `old_session_id_matches_current` guards at `meerkat-mob/src/machines/mob_machine.rs:1049+`). Catching assertion: `grep -rn "MemberSessionBindingSet\|MemberSessionBindingRotated\|MemberSessionBindingReleased" meerkat-*/src/` returns zero hits.

4. **C-WIRE-RETIRED leaves dangling method names in the RPC catalog.** Catching assertion: `verify-schema-freshness` extended to assert `rpc_catalog` contains no `session/{retire,reset,submission,submissions}` entries.

5. **C-12 ad-hoc CLI parser regrowth.** Catching assertion: `meerkat-cli/tests/connection_ref_single_parser.rs` asserts exactly one **`ConnectionRef`-parsing** call site (scoped by function name and wrapping module, not a byte-pattern match on `split_once(':')` — `main.rs:3798` parses `TokenKey` and `mcp.rs:193` splits HTTP headers, neither is ConnectionRef). Adversarial review flaw 7.

6. **C-AGENT-RT-ID `pub(crate)` tightening breaks an out-of-tree consumer.** Catching assertion: `rmat-audit` run at end of C-7 + `meerkat-mob/tests/public_api.rs` instantiates `MobMemberSnapshot` externally and confirms the field is unaddressable.

7. **C-REST-READMUT deletion silently loses a lazy registration side-effect.** Trace `SessionRuntime::ensure_registered` call sites; if zero non-READ callers survive, delete the helper entirely.

8. **V5 `PeerName`-keyed maps leak outside `meerkat-comms`.** Catching assertion: `rg 'HashMap<PeerName|BTreeMap<PeerName' --type rust` returns zero hits outside display-only comms sites.

9. **Composition dispatcher becomes an "optional fast path"** (LATE FINDING C risk). If C-6p wires producers but direct calls survive next to dispatcher calls, the dispatcher is not THE path. Catching assertion: B-10's `CompositionDispatchIsThePath` rule is live in `rmat-audit --strict` (`xtask/src/rmat_audit.rs:614`); `CompositionRouteSemanticCoverage` (at `:488`) enforces typed route agreement. The 87 handle-layer `apply_input` sites (see C-6c expanded allowlist) are the primary regression surface for this risk.

10. **Duplicate DSL drift (mob composition emit vs schema catalog).** The two `mob_machine.rs` files at `meerkat-mob/src/machines/mob_machine.rs` (2654 lines, runtime-authoritative) and `meerkat-machine-schema/src/catalog/dsl/mob_machine.rs` (1387 lines, TLC-focused schema mirror) are **intentionally different** — not duplicates awaiting consolidation. The existing `machine-check-drift` gate (`xtask/src/machines_test_support.rs:198`) handles actual semantic drift. The real risk is the `PeerEndpoint` twin at `meerkat-runtime/src/meerkat_machine/dsl.rs:1721` and `meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs:3037`, which carry an in-line comment mandating structural equivalence. Catching assertion: `xtask::machines::check_dsl_parity` + `verify-schema-freshness` both pass post-C-6r.

11. **PeerName/AgentIdentity roster conflation surviving wave-c** (LATE FINDING A — verified stale as of HEAD). `From<PeerName> for AgentIdentity` has already been deleted workspace-wide; only `impl From<PeerName> for String` at `meerkat-core/src/comms.rs:198` remains. `meerkat-mob/src/roster.rs:86,102` already types `wired_to: BTreeSet<AgentIdentity>` and `external_peer_specs: BTreeMap<AgentIdentity, TrustedPeerSpec>` separately. C-7r is a verify-and-close task rather than a retype. Risk rescoped: residual coercion path is re-introduced by an unaudited handler. Catching assertion: `grep -rn "AgentIdentity::from" meerkat-mob/src/` returns only test/fixture constructor sites.

12. **Test coverage attrition.** 97 tests lost across wave-a; 6 are "Lost and blocker" per `test-coverage-audit.md` §5.3 (trust-reconcile concurrency, respawn overlay rotation, shadow-mode parity, release-binding sweep, mob restore-failure classification, turn-boundary-denial). §6 specifies a 16-item minimum rebuild list mapping to these blockers plus 10 "concerning" behaviors. Catching assertion: the C-T task list above names every blocker test by file path; items 10-16 are either specified as new tests or deleted with a one-line justification per §6 completion criterion.

13. **Persistence migration failure modes.** Worst case — a v0 `connection_ref` slug fails regex even after slugification. Mitigation: `migrate(Value) -> Result<Session, SessionMigrationError>` with `SessionMigrationError::Partial` preserving the legacy payload under `legacy_connection_ref`. No outright session loss.

14. **Realtime rotation pinning on broadcast `Lagged` (audit §3.3).** C-R8 closes this by re-resolving the binding map on `Lagged`, not just polling attachment status. Catching assertion: realtime WS test that injects synthetic `Lagged` followed by out-of-band binding change and asserts the socket converges to the canonical binding.

15. **DSL stringly-typed PeerEndpoint residue** (LATE FINDING B). If C-6r doesn't retype BOTH `meerkat-runtime/src/meerkat_machine/dsl.rs:1721-1757` AND its structural twin at `meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs:3037`, typing evaporates at the catalog seam and B-8's wave-b work regresses. The in-line comment at `dsl.rs:1715-1720` mandates structural equivalence. Catching assertion: grep `name: String` inside `PeerEndpoint`-adjacent files returns zero hits AND `xtask::machines::check_dsl_parity` passes (adversarial review R-A).

---

## Section 6 — Completion criteria

Wave (c) is done when all hold:

- `./scripts/repo-cargo check --workspace --all-targets` exits zero.
- `./scripts/repo-cargo nextest run --workspace` passes with zero failures.
- `./scripts/repo-cargo clippy --workspace -- -D warnings` passes.
- `git diff origin/main..HEAD -- '*.rs' | grep '^+.*#\[allow(dead_code)\]'` returns zero (no new allow).
- No new `serde_json::Value` fields in semantic seams. The only surviving `Value`s in `meerkat-contracts/src/wire/` and `meerkat-core/src/lifecycle/run_primitive.rs` are the allow-listed pass-through fields (structured_output, params_schema, MCP server_config, tool-call args as `Box<RawValue>`). `meerkat-contracts/src/wire/session.rs` `args: Value` fields are audited and confirmed allow-listed pass-through.
- Every tombstone cluster in the base draft §1 returns zero `rg` hits against its deleted-symbol fingerprint, except the `pub(crate)` survivors in C-AGENT-RT-ID.
- `rmat-audit --strict` passes, including `CompositionDispatchIsThePath` and `CompositionRouteSemanticCoverage` rules (B-10 backstops). Verified via synthetic-violation injection test.
- All 6 blocker tests from `test-coverage-audit.md` §5.3 exist under the file names enumerated in C-T. Each of the 10 concerning behaviors (§6 #10-16) is either covered by a new test that C-T specifies, or deleted in this wave with a one-line justification per test in the merge commit. Post-hoc "consciously replaced" is not a valid completion claim; the justification must be written and reviewer-challengeable before wave-c closes.
- Persistence migration validated: fixtures 1-11 under C-3 (`meerkat-session/tests/persistence_compat.rs`), fixture #12 under C-6r (`meerkat-runtime/src/store/sqlite.rs` read path).
- `SessionStore` is type-level append-only (F1 from state-scope-audit.md): no `save()` call can shorten a session's message history under a stable `SessionId`. `Session::fork_at` returning a new `SessionId` is ALREADY the case pre-wave-c (verified at `session.rs:803`); `meerkat-core/tests/fork_at_rotates_session_id.rs` guards against regression. F7 is closed by the type-level append-only witness on `Session.messages` + the `AppendOnlySessionStore` trait guarding `save()`.
- `RosterEntry.wired_to` audit closes clean (verify-and-close per C-7r; zero residual `AgentIdentity::from(peer_name)` coercion paths).
- `PeerEndpoint` in DSL carries `PeerId`/`PeerAddress`/`PeerName` — no `String` fields; both copies (`meerkat-runtime/src/meerkat_machine/dsl.rs:1721` and `meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs:3037`) retyped in lockstep.
- C-F2 (trust-edge mutation authority) and C-F3 (mob-destroy → ingress-detach obligation) are landed; seam-inventory records both as protocolized handoffs.
- C-WAR items WAR-1/WAR-2/WAR-3 clean (pre-existing wave-a residue surfaced by `rmat-audit --strict`).
- **C-REGEN run:** `make regen-schemas` committed; `verify-schema-freshness` passes; retired verbs removed from catalog; `sdks/python/meerkat/generated/` and `sdks/typescript/src/generated/` match HEAD contracts.
- `make verify-version-parity` passes (schema-side only; SDK package version bumps remain wave-d).
- `make ci` exits zero locally. GitHub `gate` green on the wave-c merge PR.
- Neither `cargo fmt --check` nor the pre-commit hook chain is skipped on the final merge commit — wave-c ends the `--no-verify` tolerance.
- **All 9 tripwires from Section 1.5 are green.** If any remain red while other completion criteria claim done, the task whose tripwire is red is not actually done. The tripwires are the tamper-resistant backstop: they were written to fail in diagnosable ways before any worker started, and their names are fixed.

---

## Section 7 — Out of scope (wave-d's problems)

Wave-d owns release infrastructure, SDK package bumps, and doc generation from the typed catalog. Schema regeneration (the mechanical pass) happens in wave-c as C-REGEN because wire types change in wave-c and the schema-freshness gate must pass for CI to go green. Wave-d boundary:

- **SDK package version bumps** (Python `sdks/python/pyproject.toml`, TypeScript `sdks/typescript/package.json`, web SDK `sdks/web/package.json`). Generated-type *content* regenerates under C-REGEN in wave-c; package *versions* are wave-d's release.
- **Release infrastructure updates** (`Cargo.toml` workspace version bump, `cargo release patch` flow, `make verify-version-parity` for lockstepped version bumps).
- **Doc regeneration** from typed catalog (`docs/reference/capability-matrix.mdx` and similar).
- **Issue #341** (fault-explicit state machines — the 11 dogma blind-spot classes). Top-5 seeds from `dogma-blind-spots.md` §7 (`ephemeral.rs:2548-2572`, `actor.rs:3320`+`actor_turn_executor.rs:269`, Anthropic timing profile, unbounded channel overflow policy, `require_peer_auth` type-state) are real but post-0.6.
- **Issue #343** (Signal-route typed dispatcher). Wave-c keeps one dispatcher shape.
- **Auth→Meerkat machine merge.** Per `machine-boundary-stability.md` §6: protocolize the seam today (under C-F2/C-6r/C-7), leave the fold itself for a later release. Leading indicator to watch: auth-state drift across the Auth/Meerkat boundary.
