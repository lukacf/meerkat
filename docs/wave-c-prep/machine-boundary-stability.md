# Machine Boundary Stability — Wave (c) Prep

Status: Analysis input for wave (c) coupling decisions. READ-ONLY survey.
Scope: The 5 canonical machines in the 0.6 catalog as of `meerkat-machine-schema/src/catalog/dsl/`.

## 0. Policy reframe (this doc's anchor)

Before any analysis: the following policy is stated, not open.

1. **No splits.** The 25+ → 5 machine reduction was the fix for systematic seam failures across the 0.5 codebase. Re-introducing machines re-introduces those failures. Split risk is therefore treated as a policy non-risk in this document; we do not score it. Any future pressure to "lift a concern out" is rejected by default.
2. **Architectural intent is two machines**: `MeerkatMachine` and `MobMachine`. The other three — `ScheduleLifecycleMachine`, `OccurrenceLifecycleMachine`, `AuthMachine` — are **tolerated as independent**, under a tacit agreement that they *may* fold into `MeerkatMachine` in a future release for architectural cleanliness. That direction of motion (merge up into Meerkat) is the only live boundary question.
3. **Verification cost is not the forcing function.** TLA+ on the current catalog runs in seconds; complexity budget is not the reason a merge would happen. Merges would be for seam / dogma cleanliness, not for model-checking cost.

Baseline prior: 1 of 16 v0.5 machines survived to 0.6 by name (`flow_run`). The chronicle's 94% non-survival rate should not be read as "splits are likely"; it should be read as "merges/collapses are the direction of travel, and we should couple wave (c) against that."

## 1. Machine sizes

| Machine | DSL LOC | Phases | Role |
|---|---|---|---|
| `MeerkatMachine` | 3059 | 8 | session-scoped execution kernel |
| `MobMachine` | 1387 | 4 | mob-scoped orchestration + topology |
| `OccurrenceLifecycleMachine` | 345 | 9 | per-occurrence dispatch |
| `ScheduleLifecycleMachine` | 224 | 3 | schedule trigger lifecycle |
| `AuthMachine` | 164 | 5 | per-binding auth lease |

Schedule + Occurrence + Auth combined = 733 DSL LOC, about one-quarter the size of MeerkatMachine today.

## 2. Per-machine concern enumeration

Kept for grounding — useful regardless of boundary policy. Citations are to `meerkat-machine-schema/src/catalog/dsl/`.

### 2.1 MeerkatMachine (`meerkat_machine.rs:9-220`)

1. Session lifecycle & identity — `session_id`, `active_runtime_id`, `active_fence_token`, `current_run_id`, `pre_run_phase`, `lifecycle_phase` (10-15).
2. Ops / turn execution — `silent_intent_overrides` (16); committed-visible-set transitions (981-1087); boundary apply (972).
3. Realtime attachment authority — `realtime_intent_present`, `realtime_binding_state`, `realtime_binding_authority_epoch`, `realtime_reattach_required`, `realtime_next_authority_epoch` (20-24); `realtime_product_turn_phase` (76); `realtime_projection_freshness` (87); `realtime_reconnect_policy` (96).
4. Live-topology reconfigure — `live_topology_phase` (27); `ReconfigureSessionLlmIdentity*` transitions (803-832).
5. MCP server authority — `mcp_server_states` (32).
6. Peer interaction lifecycle — `pending_peer_requests`, `inbound_peer_requests` (41-43).
7. Session context advancement — `last_session_context_updated_at_ms` (53).
8. Interaction stream lifecycle — `reserved_interaction_streams`, `attached_interaction_streams` (66-67).
9. Peer-ingress transport capability — `peer_ingress_owner_kind` and companions (110-112).
10. Supervisor-bridge authorization — `supervisor_binding_kind` and companions (130-134).
11. Track-B peer projection — `local_endpoint`, `direct_peer_endpoints`, `mob_overlay_peer_endpoints`, `peer_projection_epoch`, `mob_overlay_epoch` (175-179).
12. Drain lifecycle — `NotifyDrainExited` (928); `EnsureDrainRunning*` (1283-1299).

### 2.2 MobMachine (`mob_machine.rs:9-50`)

1. Mob lifecycle (72-76).
2. Member lifecycle / runtime roster (11-21).
3. Coordinator binding (16; signals 151-152).
4. Run accounting (14-15).
5. Identity↔runtime map (29).
6. Wiring graph (25).
7. Member-session bindings + topology epoch (48-49).
8. Task board (32-36).
9. Flow/frame/loop execution — via compat kernels `FlowRunMachine`, `FlowFrameMachine`, `LoopIterationMachine` in `meerkat-machine-schema/src/compat/`.

### 2.3 OccurrenceLifecycleMachine (`occurrence_lifecycle.rs:8-63`)

1. Occurrence lifecycle phase — 9 phases; terminal `[Completed, Skipped, Misfired, Superseded, DeliveryFailed]`.
2. Claim / lease management (15-19).
3. Dispatch correlation (20-25).
4. Failure classification (22-26).
5. Revision / supersede (12-27).
6. Targeting (14-15).

### 2.4 ScheduleLifecycleMachine (`schedule_lifecycle.rs:8-37`)

1. Schedule phase — Active / Paused / Deleted.
2. Trigger key (11).
3. Target binding (12).
4. Policies — misfire / overlap / missing-target (13-15).
5. Planning window (16-17).
6. Revision (10).

### 2.5 AuthMachine (`auth_machine.rs:24-50`)

1. Lease phase — Valid / Expiring / Refreshing / ReauthRequired / Released.
2. Expiry watermark (31).
3. Refresh bookkeeping (32-33).

Per-binding instance, <200 LOC including transitions. The header (3-23) documents that absorption into MeerkatMachine was tried, reviewed, and reversed — but the review rationale was "orthogonal per-binding fact," which is a *preference* argument, not a *forcing* one. It remains a merge candidate.

## 3. Split risk — out of scope

Per the policy in §0, split risk is not analyzed. Any proposal that moves a concern out of `MeerkatMachine` or `MobMachine` into a new machine is rejected by default. The concern enumeration in §2 exists to ground merge analysis and to inform wave (c) coupling — not to argue for future fission.

## 4. Merge analysis (the live question)

Architectural intent: 2 machines (`MeerkatMachine`, `MobMachine`). Today we have 5. Score merge likelihood for the three tolerated-independents, and enumerate the criteria that would actually flip the decision.

### 4.1 AuthMachine → MeerkatMachine

**Merge likelihood: Medium.**

Signal for merge:
- Tiny (164 LOC, 3 stateful fields, 5 phases, 1 cross-machine routed effect absent).
- Already attempted once; reversal rationale (`auth_machine.rs:3-23`) is "orthogonal per-binding fact," which is a *preference* argument rooted in TLC state-space pressure — and §0 says TLC cost is no longer the forcing function.
- AuthMachine has a per-binding cardinality (`<realm_id>:<binding_id>`) that `MeerkatMachine` today does not natively model per-session. That's a real data-model friction. But `MeerkatMachine` already carries maps keyed on composite IDs (`mcp_server_states: Map<McpServerId, ...>`), so the pattern is precedented.
- No shell-side public-surface coupling today — see §5.3: only schema / catalog / tests reference the type. A merge is therefore a low-blast-radius change.

Signal against merge:
- Per-binding instance lifecycle is genuinely orthogonal to session lifecycle (auth can expire while no session is running).
- The original review explicitly landed on "keep separate." That decision should be honored unless a forcing signal appears.

**Concrete criteria that would trigger a merge decision:**
1. A dogma violation where auth-lease truth and session truth drift (e.g. a shell holds "this session has valid auth" while the AuthMachine says `ReauthRequired`). That's the classic "one semantic fact, one owner" (dogma §1) break that forces collapse.
2. A new feature that requires atomic state transitions across auth and session (e.g. "pause all sessions on this binding while lease refreshes"). Two-machine atomicity is expensive; merged-machine atomicity is a single transition.
3. AuthMachine acquiring >1 cross-machine routed effect to MeerkatMachine.
4. Auth state escaping into `MeerkatMachine` shadow fields — i.e. the 0.5-style seam failure that motivated the collapse in the first place.

### 4.2 ScheduleLifecycleMachine + OccurrenceLifecycleMachine → MeerkatMachine

**Merge likelihood: Low (as a pair); Very low (individually).**

Signal for merge:
- Combined 569 DSL LOC. Not a complexity barrier.
- Both ultimately deliver into session/mob — their outputs become `MeerkatMachine` inputs.

Signal against merge:
- Cardinality is fundamentally wrong for folding into `MeerkatMachine`. Schedules/occurrences are runtime-level, not session-scoped. A `MeerkatMachine` instance exists per session; schedules outlive sessions, fire when no session is attached, and target `target_binding_key`s that may resolve to zero or many sessions.
- Schedule→Occurrence is already modeled as cross-machine routing (`meerkat-schedule/src/machines/schedule_lifecycle.rs:82`: `disposition SupersedePendingOccurrences => routed [OccurrenceLifecycleMachine]`). That routing is explicit about a 1:N parent-child relationship.
- Folding them into `MeerkatMachine` would require `MeerkatMachine` state to carry schedule maps and occurrence maps. That is not a cleanliness win — it mixes per-session and cross-session state in one machine, which is precisely what the 0.5→0.6 collapse was trying to avoid.

**If merge happens, it probably isn't into MeerkatMachine.** A more coherent merge would be `ScheduleLifecycleMachine + OccurrenceLifecycleMachine → ScheduleMachine` (one machine, per-schedule instance, occurrences modeled as an inner map). That stays within the "fewer machines" policy while respecting cardinality.

**Concrete criteria that would trigger a (Schedule+Occurrence) → Schedule merge:**
1. Durable evidence of a seam failure across the Schedule/Occurrence boundary (e.g. an occurrence lingering after its schedule is deleted, or a revision race).
2. `SupersedePendingOccurrences` acquiring siblings — i.e. the Schedule→Occurrence routing contract growing to >3 routed dispositions, at which point the routing itself becomes a shadow machine.
3. Planning-window bookkeeping (`planning_cursor_utc_ms`, `next_occurrence_ordinal`) needing per-occurrence reconciliation that's hard to express across two machines.

**Criteria that would trigger a (Schedule or Occurrence) → MeerkatMachine merge:** essentially none that are plausible today. Would require eliminating the runtime-level scheduler and making schedules session-scoped, which is a product-level pivot, not an architectural cleanup.

### 4.3 Summary of merge pressure

| Merge | Likelihood | Blocking factor |
|---|---|---|
| Auth → Meerkat | Medium | Per-binding vs per-session cardinality; prior rejection |
| Schedule → Meerkat | Very low | Cardinality (runtime vs session) |
| Occurrence → Meerkat | Very low | Cardinality |
| Schedule + Occurrence → one Schedule machine | Low | No forcing signal today, but the cleanest consolidation if pressure appears |

Net: `Schedule` and `Occurrence` are stable as two independents. `AuthMachine` is the one with a non-trivial chance of folding up.

## 5. Current shell-level coupling

### 5.1 MeerkatMachine struct-name use (237+ sites)

Constructor sites `MeerkatMachine::ephemeral()` / `MeerkatMachine::persistent(...)` across non-target, non-test code. Highest-density live offenders:

- `meerkat-cli/src/main.rs` — 14 occurrences; 5 constructor sites (`main.rs:5250, 9398, 9463, 9904, 10066`).
- `meerkat-rpc/src/router.rs` — `router.rs:2539` constructs `MeerkatMachine::persistent(...)` in the RPC surface.
- `meerkat-rest/src/lib.rs` — 3 occurrences.
- `meerkat-mob/src/runtime/local_bridge.rs` — 7 constructor sites (lines 252, 265, 278, 297, 316, 335, 350).
- `meerkat-mob-mcp/src/` — 5 constructor sites across `surface.rs`, `lib.rs`, `agent_tools.rs`.
- `meerkat/src/service_factory.rs` — 2 sites (lines 719, 920).
- `meerkat-openai/src/realtime_attachment.rs` — 3 sites.

These are **fine** under the current policy — shell may tightly couple to the two-machine set. But see §6 for one cheap hedge.

### 5.2 MobMachine

Only `meerkat-mob/src/runtime/builder.rs` and `.../handle.rs` import `MobMachine` directly. Tight coupling, narrow blast radius.

### 5.3 Schedule / Occurrence / Auth

- `meerkat-schedule/src/lifecycle.rs` uses `sched_dsl::ScheduleLifecycleMachineAuthority` / `occ_dsl::OccurrenceLifecycleMachineAuthority` directly (lines 188-720). One crate, scoped.
- `meerkat-machine-schema/src/lib.rs:61-79` hardcodes the names `"ScheduleLifecycleMachine"` and `"OccurrenceLifecycleMachine"` in composition contract checks.
- `AuthMachine` is referenced only by schema, catalog, tests, and `tests/integration/tests/e2e_auth_lane.rs:190`. **No surface crate hardcodes its name.** This is the merge candidate most pre-adapted for the move — folding its 3 fields into `MeerkatMachine` would touch `meerkat-runtime/src/handles/auth_lease.rs` (the registry) but leave every surface crate untouched.

### 5.4 RMAT / xtask policy tables

`xtask/src/ownership_ledger.rs` has 28 string-literal `"MeerkatMachine"` entries; `xtask/src/rmat_policy.rs:191-193` hardcodes `("MobMachine", input, "MeerkatMachine")` tuples for routed-disposition rewrites. These are policy code, not shell, but they encode identity as strings. A future Auth → Meerkat merge would need a single pass here.

## 6. Wave (c) coupling recommendations

Biased toward "couple tightly to the 5 machines, keep one cheap escape hatch."

1. **Shell code may couple tightly.** The policy says no splits, so the 237-site `MeerkatMachine::ephemeral()` footprint is acceptable. Don't over-engineer split-resilience.
2. **One cheap hedge: typed `MachineId` lookup where the cost is near zero.** Wave (b) landed the typed `MachineId` newtype (`meerkat-machine-schema/src/identity.rs:130`). Shell code that builds lookup tables keyed on machine identity (RMAT policy, xtask ownership ledger, composition contract checks) should use `MachineId` rather than `&str`. That way, an Auth → Meerkat fold is `s/MachineId::parse("AuthMachine")/MachineId::parse("MeerkatMachine")/` at a small set of call sites rather than a rewrite of dispatch.
3. **Per-surface constructor funnels.** The CLI has 5 `MeerkatMachine::ephemeral()` sites; RPC, REST, mob-mcp each have their own. Wave (c) should consolidate per-surface construction to one helper (`service_factory::build_meerkat_machine(...)` already partially exists). If Auth merges into Meerkat, the signature change hits 1 site per surface rather than 14.
4. **Keep machine identity off the public wire.** Audit `meerkat-contracts` in wave (c) to ensure no SDK/JSON-RPC wire type mentions `MeerkatMachine` / `MobMachine` / etc. by name. SDK clients should never be able to build code against machine identity. Not exhaustively scanned in this pass — flag for wave (c) implementers. (A merge should never be a public-wire breaking change.)
5. **Protocolize the Auth↔Meerkat seam now.** Since Auth is the live merge candidate, wave (c) should explicitly model the seam between `AuthMachine` and `MeerkatMachine` as a formal handoff (the `docs/architecture/formal-seam-closure.md` pattern). That does two things: (a) it closes the §4.1 trigger condition #1 (state drift) by design; (b) if a future merge does happen, the seam *becomes* internal transitions of `MeerkatMachine` with no surface change.
6. **No new machines in wave (c).** Direct corollary of §0. If wave (c) implementers reach for "let's lift X into its own machine," the review answer is "no — extend `MeerkatMachine` or `MobMachine`."

## 7. Leading indicators for merge decisions

What to watch in the coming releases to decide if a merge is justified. These are merge-triggering signals, not split-predicting ones (per §0).

1. **State drift between a sibling machine and its parent.** If `AuthMachine` lease truth diverges from whatever `MeerkatMachine` believes about auth validity — even for one session — that is a dogma §1 break and the strongest merge forcing function.
2. **Cross-machine routed-disposition count.** Today `ScheduleLifecycleMachine` has exactly one routed effect to Occurrence (`SupersedePendingOccurrences`, `meerkat-schedule/src/machines/schedule_lifecycle.rs:82`). If that grows to ≥3, the routing contract is itself a shadow machine — merge (likely Schedule + Occurrence) is the cleanup.
3. **Shell fields that mirror sibling-machine state.** If audit finds handwritten shell code caching `AuthLifecyclePhase` values or polling `AuthMachineAuthority` to synthesize session-level decisions, the machine is paying the cost of separation without the semantic benefit. Merge.
4. **Cardinality coincidence.** Today `AuthMachine` is per-binding, `MeerkatMachine` per-session. If a realm configuration ever forces 1:1 auth:session for a meaningful subset of deployments (e.g. user-session-bound OAuth), the cardinality argument against merge weakens.
5. **Epoch namespace proliferation inside MeerkatMachine or MobMachine.** Already 4+ independent epoch namespaces live inside `MeerkatMachine` (`realtime_next_authority_epoch`, `peer_projection_epoch`, `mob_overlay_epoch`, `supervisor_bound_epoch`). Read this as: the parent machines are *already* absorbing the state a split would have lifted out. This is the policy working; track whether a new epoch family appears with each release.
6. **Surface-only input growth.** `machine-simplification-proposal.md` documents 40+ Meerkat inputs reclassified as `surface_only`. Continued growth means the machine is becoming an API facade, which is evidence the fact population inside it is stable (good — nothing to merge into from outside) or evidence that it is attracting accidental API surface (neutral).
7. **TLC green continues at seconds-scale.** Per §0, this is not a forcing function for further collapse, but a regression (TLC going from seconds to minutes) would be the one signal we *don't* want to ignore — it would indicate state inflation that simplification passes should address.

Track items (1), (2), (3) routinely; they are the ones that would actually flip a merge decision.

## Summary

Per the stated policy, splits are rejected by default; the only live boundary question is whether `AuthMachine`, `ScheduleLifecycleMachine`, or `OccurrenceLifecycleMachine` should later fold up toward the 2-machine architectural intent. `AuthMachine` is the one with non-trivial (medium) merge likelihood: it's tiny, already tried once, has no surface-crate coupling, and could be absorbed into `MeerkatMachine` with a single-pass touch. `ScheduleLifecycleMachine` + `OccurrenceLifecycleMachine` are unlikely to fold into `MeerkatMachine` at all (cardinality is wrong); if they merge, it's with each other into one `ScheduleMachine`, driven by routed-disposition growth across their shared seam. Wave (c) should let shell code couple tightly to the current 5-machine set, route composition-layer and xtask-policy identity through typed `MachineId` (near-zero cost), funnel per-surface constructors to one site per surface, keep machine identity off the public wire, and protocolize the Auth↔Meerkat seam today so a future fold lands as internal-to-Meerkat transitions with no surface break. The leading indicator to watch is auth-state drift across the Auth/Meerkat boundary — that's the one signal that would turn the medium-likelihood merge into a forced one.
