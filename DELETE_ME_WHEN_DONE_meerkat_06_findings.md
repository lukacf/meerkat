# Meerkat 0.6 (codex/realtime-voice) — Verified findings

Verified against worktree `/Users/luka/.codex/worktrees/ae76/meerkat` on branch `codex/realtime-voice` (HEAD), 93 commits ahead of `main`.

## Status (updated as fixes land)

- ✅ **A1** — `mob/lifecycle destroy` surfaces `MobDestroyReport` (commit `07dd9d67f`, regression in `meerkat-rpc/tests/regression_rpc.rs::mob_create_status_list_lifecycle`).
- ✅ **A7** — `MemberRef` is now `pub(crate)` (was `#[doc(hidden)] pub`); `ExternalBindingOverlayRecord.normalized_member_ref` follows. No external crate imports either today.
- ✅ **B7** — `MemberSpawnedEvent.runtime_mode` default is load-bearing: pre-0.6 persisted events predate the field, and the `AutonomousHost` default matches pre-field semantics. Intentional coercion is now documented inline on the field, and `mob_event_legacy_member_spawned_runtime_mode_defaults_to_autonomous_host` regression pins it so a future schema change cannot silently flip the default.
- ✅ **A10 (partial — binding / shell_env / auto_wire_parent)** — RPC `MobSpawnParams` now accepts `binding: Option<WireRuntimeBinding>`, `shell_env: Option<HashMap<...>>`, and `auto_wire_parent: Option<bool>`. Partial per the finding note; remaining fields (`tool_access_policy`, `budget_split_policy`, `launch_mode`, `inherited_tool_filter`, `override_profile`) need their own wire contracts and are tracked for a follow-up.
- ✅ **C6** — `WorkSpec.content` is now `meerkat_core::types::ContentInput` (was `String`); `WorkSpec::new` accepts `impl Into<ContentInput>` so existing text-only call sites upgrade without churn. Matches every other content-carrying type on the platform. Regression `test_work_spec_accepts_multimodal_content` locks in that non-text ContentInput variants (e.g. image blocks) can be submitted as work content.
- ✅ **C2** — `mob/snapshot` RPC lands (handler `meerkat-rpc/src/handlers/mob.rs::handle_snapshot`, catalog entry, router wiring). Returns `{mob_id, status, members[]}` in one atomic call so consumers do not have to compose `mob/status` + `mob/members` or fall back to event-stream projection for point-in-time state. Regression extends `mob_create_status_list_lifecycle` to assert the response shape.
- ✅ **B5** — Audit: `internal_turn`, `mob/turn_start`, and `mob/member_send` are NOT three names for the same operation. Rust `internal_turn` writes directly into the member's pending-turn slot (no peer comms, no handling-mode). RPC `mob/turn_start` resolves the identity to the bridge session and delegates to the canonical `turn/start` handler (turn-level overrides). RPC `mob/member_send` is peer-delivery shape (`HandlingMode` + `RenderMetadata`) into the comms inbox, not a new turn. Each has a distinct authorization story and return shape; collapsing them would erase real policy. Documented on `MobHandle::internal_turn` so the distinction is self-describing; no code change required.
- ✅ **C10** — `mob/rotate_supervisor` RPC landed (handler `meerkat-rpc/src/handlers/mob.rs::handle_rotate_supervisor`, catalog entry, router wiring). Response body carries the full `SupervisorRotationReport` so operators can inspect per-member rotation outcomes. Regression in `meerkat-rpc/tests/regression_rpc.rs::mob_create_status_list_lifecycle` asserts the method is registered (is-not-method-not-found).
- ✅ **C12** — `mob/events` already supports cursor-based replay today: `MobEventsParams { mob_id, after_cursor: u64, limit: usize }` at `meerkat-rpc/src/handlers/mob.rs:599-606`. No code change required; finding was a mis-assumption. Documentation audit closes the item.
- ✅ **C3** — Dedicated `mob/destroy` RPC lands (handler `meerkat-rpc/src/handlers/mob.rs::handle_destroy`, catalog entry, router wiring). Returns `{mob_id, ok, destroy_report}` so callers get the structured `MobDestroyReport` without having to branch on `mob/lifecycle` action strings. Regression `meerkat-rpc/tests/regression_rpc.rs::mob_destroy_rpc_surfaces_report` creates a mob, calls `mob/destroy`, asserts the destroy_report object is present and the mob disappears from `mob/list`.
- ✅ **A6** — `MemberSpawned(MemberSpawnedEvent)` variant shape difference is intentional and now documented inline on the variant: the wrapped struct is load-bearing because `MemberSpawnedEvent.bridge_member_ref` is `#[serde(skip)]` crate-internal replay metadata that must not leak onto the public wire. "Shell owns mechanics, not meaning" — inline variant fields would force the replay plumbing into the public event enum. Regression `member_spawned_public_wire_shape_excludes_bridge_member_ref` pins the public serialized shape so a future refactor cannot silently promote the internal pointer into the public contract.
- ✅ **A8** — `resolve_bridge_session_id` is not a dogma contradiction; it is a shell-mechanics escape hatch explicitly permitted by `meerkat-runtime-dogma.md` principle #3 ("shell owns mechanics, not meaning"). The apparent contradiction was a scoping conflation: identity-first hides session_id from **consumers of the public mob surface** (application code, SDK clients), while surface implementations (RPC/MCP/REST handlers, web-runtime wrappers, delegation tools) must still bridge identity to session when delegating to session-scoped canonical APIs they don't own. The method reads the canonical roster mapping — no parallel truth. The 15+ existing call sites (`meerkat-rpc/src/handlers/mob.rs::handle_mob_turn_start`, `meerkat-mob-mcp` delegation tools, `meerkat-web-runtime`, `smoke_mob_resume.rs` tests) already exercise the mechanism. Fix: removed `#[doc(hidden)]` since the method IS a public surface for shell implementations; added a "when to use / when not to use" doc block citing the canonical dogma principle and pointing end-user code toward the identity-native [`MobHandle`] alternatives (`member`, `internal_turn`, `peer_send`, `member_send`).
- ✅ **B4** — `rotate_supervisor(&self)` signature has no scoping parameter because the supervisor is unambiguously **mob-wide**: a single `SupervisorAuthorityRecord` persisted by `mob_id`, rotated once, broadcast to every remote member binding on the roster. Per-member `BridgeBootstrapToken`s carried on `MemberRef::BackendPeer` are bootstrap proofs authorizing a specific member's bridge to (re)establish under the current supervisor, NOT a separate per-member supervisor identity. One supervisor, many bootstrap tokens. Finding B4's ambiguity was in the doc, not the signature; per dogma principle #1 ("one semantic fact, one owner") the signature already matches the data model. Fix: expanded `MobHandle::rotate_supervisor` doc with scope, partial-failure semantics, and a pointer to the 4 existing regression tests in `meerkat-mob/src/runtime/tests.rs` (`test_rotate_supervisor_updates_runtime_metadata`, `..._reauthorizes_live_remote_members_and_rejects_stale_epoch`, `..._bind_fallback_binds_next_authority`, `..._advances_local_authority_when_rollback_fails`).
- ✅ **B3** — `reset()` returning `Result<(), MobError>` and `destroy()` returning `Result<MobDestroyReport, MobDestroyError>` is a load-bearing asymmetry that matches the underlying member-teardown shape: reset uses `retire_all_members` (idempotent, all-or-retry — cleanup errors are best-effort, reissuing reset retries safely) while destroy uses `destroy_all_members_for_destroy` (force-fallback with structured partial-cleanup report). Reset by design avoids the force-fallback regime, so it has no equivalent structured partial-outcome data to surface; a `MobResetReport` would be fictional. Per dogma principle #5 ("typed truth, never string folklore") reset's `()` return correctly signals "clean or retry"; inventing a report would be parallel truth. Fix: expanded `MobHandle::reset` doc with a side-by-side comparison vs destroy (scope, teardown model, return type, partial outcomes, event marker, post-op handle usability) and cites dogma principles #1 and #5. Regression coverage in `test_reset_clears_roster_events_and_returns_to_running`, `test_reset_allows_spawn_after_reset`, and the supervisor-escalation reset tests.
- ✅ **A5 (incremental — hot-path conversions)** — Added `impl From<AgentIdentity> for MeerkatId` and `impl From<&AgentIdentity> for MeerkatId` so identity-first shell code writes `MeerkatId::from(identity)` / `MeerkatId::from(&identity)` instead of `MeerkatId::from(identity.as_str())`. 20+ call sites in `meerkat-mob/src/runtime/handle.rs` updated. Regression `agent_identity_to_meerkat_id_conversion_preserves_identity_string` pins the representation-change (not semantic-transform) nature of the conversion. Inline doc on the new impls names this as a migration boundary — `MeerkatId` is still the identifier carried inside generated `MobMachineCommand` variants because the DSL schema has not yet been flipped to `AgentIdentity`; the full migration (schema update + kernel regeneration + handler retyping) remains tracked as the dogma-critical piece of A5. What this incremental fix delivers: the `.as_str()` gymnastics are gone and the boundary is explicit in `ids.rs` so future migration can replace it call-site-by-call-site without rewriting every caller. Remaining A5 work (DSL schema migration) stays on the "dogma-critical" list.
- ✅ **A2 + B8** — `MobError` variants `MeerkatNotFound` / `MeerkatAlreadyExists` renamed to `MemberNotFound` / `MemberAlreadyExists`; display strings renamed from "meerkat not found: X" to "mob member not found: X" (same for the other variants). 47 call sites across the workspace (meerkat-mob, meerkat-rpc, meerkat-web-runtime integration, tests) updated in one pass. Inner types remain `MeerkatId` until A5's full DSL-schema migration flips them to `AgentIdentity`; the rename lands first so public error matching no longer leaks the legacy "meerkat" term (which is what findings A2 + B8 actually flagged — the identity-first cascade missed the error module's public wording). Regression `member_not_found_and_already_exists_use_identity_first_display` pins both the display strings and the absence of the legacy term so a future refactor cannot regress the cascade.
- ✅ **A9** — `MobDestroyError::Incomplete { report }` no longer forces callers to match on `Err`; the state wrapper returns `Ok(report)` so partial-cleanup reports are read the same way as clean destroys (commit `310905c77`; covered transitively by the A1 regression).
- ✅ **B1 / C11** — `MobMemberListEntry.realtime_attachment_status` landed (commit `93e5aab10`).
- ✅ **B2 / C8** — `voice_intent_present` exposed on `MobMemberSnapshot` and `MobMemberListEntry` (commit `93e5aab10`, regression in `meerkat-mob/src/runtime/handle.rs::tests::mob_member_snapshot_exposes_agent_identity_convenience_and_voice_intent`).
- ✅ **C9** — `MobMemberSnapshot::agent_identity()` convenience landed (commit `93e5aab10`, same regression as B2/C8).
- 📌 Remaining: A3, A4, A5 (DSL-schema migration), A10 (rest), B6, C1, C4, C5, C7 — all tracked, all require regression coverage per the "extensive regression tests" standard. Priority order:
    - P0 (dogma-critical): A4 (inert DSL voice-intent field), B6 (two-kernel voice-intent overlap), A5-DSL (full schema migration from `MeerkatId` → `AgentIdentity` inside `MobMachineCommand` variants).
    - P1 (API hygiene / migration correctness): A3 + C1 (no public session-adoption path), A10 (MobSpawnParams drops 5 remaining spec fields).
    - P2 (surface completeness): C4, C5, C7.

The fix approach must stay dogma-aligned per `docs/architecture/meerkat-runtime-dogma.md`: typed truth (not string folklore), one semantic owner per fact, derived projections never authoritative, surfaces are skins not authorities. Each fix lands with a regression test that would have caught the original issue before the fix.

---

## a) Inconsistencies / errors

### A1. `mob/lifecycle destroy` RPC discards `MobDestroyReport`
`MobHandle::destroy()` returns `Result<MobDestroyReport, MobDestroyError>` with 7 structured fields (force_destroyed_members, orphaned_remote_members, errors, …).
- `meerkat-mob/src/runtime/handle.rs:2411`

But the MCP state wrapper throws it away:
- `meerkat-mob-mcp/src/lib.rs:527` — `match managed.handle.destroy().await { Ok(_report) => { … } Err(error) => Err(MobError::Internal(format!("mob destroy failed: {error}"))) }`

Then the RPC handler returns `{"ok": true}`:
- `meerkat-rpc/src/handlers/mob.rs:456–462`

**Net effect:** the whole point of the structured report is invisible to every RPC client, including mobkit.

### A2. `MobError` still exposes `MeerkatId` publicly in variant names and payloads
After identity-first migration, the public error surface is still session-era:
- `meerkat-mob/src/error.rs:17` — `MeerkatNotFound(MeerkatId)` (variant name literally "Meerkat")
- `:21` — `MeerkatAlreadyExists(MeerkatId)`
- `:25` — `NotExternallyAddressable(MeerkatId)`
- `:41` — `MemberRestoreFailed { member_id: MeerkatId, session_id: Option<SessionId>, reason }`
- `:48` — `KickoffWaitTimedOut { pending_member_ids: Vec<MeerkatId> }`

Any mobkit user matching on `MobError` sees `Meerkat`-prefixed variants and has to know about the deprecated ID type.

### A3. `MemberLaunchMode::Resume` and `Fork` are unreachable from outside `meerkat-mob`
- `meerkat-mob/src/launch.rs:18` — `pub(crate) enum MemberLaunchMode { Fresh, Resume { bridge_session_id }, Fork { source_member_id, fork_context } }`
- `meerkat-mob/src/runtime/handle.rs:568` — `pub(crate) launch_mode: MemberLaunchMode`
- `:646` — `pub(crate) fn with_resume_bridge_session_id`
- `:656` — `pub(crate) fn with_launch_mode`

**Net effect:** there is no public path to resume a bridge session or fork from another member — mobkit's `attach_existing_session` breaks at the bump.

### A4. MobMachine DSL declares a `member_voice_intent` field that the runtime cannot populate
- Commit `9e08d7580` ("honest comment about inert member_voice_intent DSL field") admits that the field is declared in the DSL catalog but never maintained at runtime because `handle_realtime_attach`/`handle_realtime_detach` take `&self` and `apply_dsl_input` requires `&mut self`.
- Canonical voice-intent state lives in `roster.voice_intent_present`, not in the DSL.
- The "MobMachine owns per-member voice intent" story doesn't hold — DSL-as-canonical is false for voice intent today.

### A5. Identity-first migration is incomplete on internal hot paths
Public signatures accept `AgentIdentity`, then convert to `MeerkatId` internally for DSL dispatch:
- `meerkat-mob/src/runtime/handle.rs:2198` — `wire`: `local: MeerkatId::from(local.as_str())`
- `:2217` — `unwire`: same
- `:2686` — `realtime_attach`: same
- `:2702` — `realtime_detach`: same
- `:2728` — `wait_for_kickoff_complete`: reads `entry.meerkat_id`, converts back to `AgentIdentity`
- `:2891` — `MemberHandle::identity(&self) -> AgentIdentity { AgentIdentity::from(self.meerkat_id.as_str()) }`

Migration is string-round-tripping, not native. DSL commands still use `MeerkatId`.

### A6. Event variant shape inconsistency
- `meerkat-mob/src/event.rs:229` — `MemberSpawned(MemberSpawnedEvent)` — wrapped struct variant.
- Every other `Member*` variant uses inline fields (`MemberRetired { agent_identity, generation, role }` at :233; `MemberReset { … }` at :245; `MembersWired { a, b }` at :266; `MemberVoiceIntentSet { agent_identity }` at :278).

**Reason:** `MemberSpawnedEvent` carries a `pub(crate) bridge_member_ref: Option<MemberRef>` at `:450` that's `#[serde(skip)]`. Externally the shape differs for a purely internal reason.

### A7. `MemberRef` is public-but-hidden
- `meerkat-mob/src/event.rs:80` — `#[doc(hidden)] pub enum MemberRef`
- Comment says "Not part of the public 0.6 mob contract — use AgentIdentity and AgentRuntimeId for all public surfaces."
- Constructors `from_bridge_session_id`, accessor `bridge_session_id()` are `pub(crate)`.
- Not used as a return type on any public `MobHandle` method (verified).

Net: the type is reachable in the crate's public namespace but effectively useless externally. Either make it `pub(crate)` or keep it as a documented public stable type.

### A8. `resolve_bridge_session_id` contradicts identity-first hiding
- `meerkat-mob/src/runtime/handle.rs:1747` — `#[doc(hidden)] pub async fn resolve_bridge_session_id(&self, identity: &AgentIdentity) -> Option<SessionId>`
- Doc says "internal routing helper for surfaces that need to call SessionService methods on a member's backing session."
- Used in `mob/turn_start` RPC handler at `meerkat-rpc/src/handlers/mob.rs:1149` — the author needs this to turn identity → session, precisely the thing identity-first tried to hide.

Either it shouldn't exist externally (and `mob/turn_start` needs a different path) or the "hide session_id from callers" principle is unenforceable.

### A9. `MobDestroyError::Incomplete { report }` is awkward to consume
- `meerkat-mob/src/runtime/handle.rs:271–274` — partial success is returned as an `Err`, forcing callers to `.or_else` or `match` just to read the report.

Every caller path (mobkit included) either ignores the report or does the match dance. Either always return `MobDestroyReport` with `errors: Vec<String>` populated, or split into "hard failure" vs "structured completion."

### A10. `MobSpawnParams` RPC cannot express most of `SpawnMemberSpec`
- `meerkat-rpc/src/handlers/mob.rs:140–156` — 8 fields.
- `meerkat-mob/src/runtime/handle.rs:556–591` — `SpawnMemberSpec` has 15 public fields.

RPC is missing: `tool_access_policy`, `budget_split_policy`, `auto_wire_parent`, `shell_env`, `inherited_tool_filter`, `override_profile`, `binding` (`RuntimeBinding`), `launch_mode`.

Net: RPC clients can't spawn with tool-access policy, budget policy, shell env, or auto-wire. Rust-in-process callers can.

---

## b) Internal consistency gaps

### B1. `MobMemberSnapshot` and `MobMemberListEntry` are divergent shapes for the same thing
- `handle.rs:58` snapshot has: status, agent_runtime_id, fence_token, output_preview, error, tokens_used, is_final, `realtime_attachment_status`, peer_connectivity, kickoff. **No `agent_identity` at top level.**
- `handle.rs:106` list entry has: `agent_identity`, agent_runtime_id, fence_token, role, runtime_mode, state, wired_to, labels, status, error, is_final, kickoff. **No `realtime_attachment_status`, `output_preview`, `tokens_used`, `peer_connectivity`.**

Caller can't iterate the roster and see voice state in one pass. Caller can't iterate snapshots and see role/labels/wired_to.

### B2. `voice_intent_present` is `pub(crate)` on every surface
- `meerkat-mob/src/roster.rs:95` — `RosterEntry.voice_intent_present: bool` is `pub(crate)`.
- Not exposed on `MobMemberSnapshot` or `MobMemberListEntry`.

Only indirect signal is `MobRealtimeAttachmentStatus` on snapshot — but status conflates intent presence with transport state (Unattached vs IntentPresentUnbound vs BindingReady). A consumer asking "does this member want voice?" has no direct answer.

### B3. Terminal operation return types diverge
- `handle.rs:2356` `stop() -> Result<(), MobError>`
- `:2369` `resume() -> Result<(), MobError>`
- `:2382` `complete() -> Result<(), MobError>`
- `:2398` `reset() -> Result<(), MobError>` — doc says "Like destroy() but keeps the actor alive"
- `:2411` `destroy() -> Result<MobDestroyReport, MobDestroyError>`

`reset` has identical failure modes to `destroy` (members may need force-cleanup, remote orphan deadlines may expire) but returns `()`. Asymmetry is real.

### B4. `rotate_supervisor()` is mob-scoped with no scoping parameter
- `handle.rs:2186` — `pub async fn rotate_supervisor(&self) -> Result<SupervisorRotationReport, MobError>`

If supervisors are per-member-bridge (plausible given `BridgeBootstrapToken` is per-MemberRef), rotating all at once is coarse. If they're mob-wide, the name could be clearer. Either way, the signature gives no choice.

### B5. `internal_turn` and `mob/turn_start` are the same operation under two names
- Rust: `handle.rs:2233` `pub async fn internal_turn(&self, identity, message) -> MemberDeliveryReceipt`
- RPC: `meerkat-rpc/src/handlers/mob.rs:1125` `handle_mob_turn_start` → resolves identity to session_id → delegates to `turn/start`

Different types, different name, same concept ("deliver content to a member by identity"). Plus `member_send` is another RPC that does this (`rpc_catalog.rs:310`). Three names for one operation.

### B6. Two-kernel realtime overlap
- `MobMachine` emits `MemberVoiceIntentSet/Cleared`; projected onto `roster.voice_intent_present`.
- `MeerkatMachine` owns `realtime_attachment_status` (transport binding state) — `meerkat-runtime/src/meerkat_machine_tests.rs:448` shows `project_realtime_attachment_intent(session_id, bool)` accepting the intent bit.

Intent flows MobMachine → shell roster → shell-calls → MeerkatMachine. The same truth is stored in three places (DSL "inert" field, roster bit, meerkat_machine intent projection). The canonical answer to "is voice enabled on this member" depends on which machine you ask.

### B7. `MobSpawnedEvent.runtime_mode` defaults silently to `AutonomousHost`
- `event.rs:443` — `#[serde(default)] pub runtime_mode: MobRuntimeMode`.

Any replay deserializing an older event that lacks runtime_mode gets `AutonomousHost` as the default. Silent semantic coercion. Either the default should fail parse, or the field should be `Option`.

### B8. Identity-first cascade missed the error module
See A2. Not just surface wording — the migration is structurally incomplete.

---

## c) Surface gaps that mobkit (and other consumers) would use

### C1. No public session-adoption API
The whole `attach_existing_session` use case has no public path today. Proposed: a public `SpawnMemberSpec::with_launch_mode(MemberLaunchMode)` with `MemberLaunchMode` made `pub`. Or dedicated `MobHandle::adopt_member(spec, session_id)`. Or a new `mob/adopt` / `mob/spawn_from_session` RPC.

Without one of these, mobkit cannot support resume or fork post-0.6.

### C2. No observational / "as-of" snapshot RPC
Mobkit today computes state by subscribing to event streams and running its own projection. The branch didn't add a simple `mob/snapshot` or `mob/roster` (returning the full `Roster`) for point-in-time state. With the DSL being canonical this is trivially free — let consumers ask rather than stitch.

### C3. `mob/destroy` deserves its own RPC
Because of A1: `mob/lifecycle action=destroy` discards the report. A dedicated `mob/destroy` RPC returning `MobDestroyReport` (and mapping `MobDestroyError::Incomplete { report }` to a structured response) is the fix. Same argument for `mob/reset` if reset ever exposes partial-cleanup info.

### C4. `mob/submit_work` / `mob/cancel_work` / `mob/list_work` RPCs
Work lane is Rust-only today. No `rpc_catalog.rs` entry. Mobkit can't offer it through the HTTP gateway. At minimum add `submit_work`/`cancel_work`; `list_work` (currently missing as Rust API too — see C5) is the observational companion.

### C5. No `list_work` / `work_status` Rust API
- `handle.rs:2290` `submit_work`, `:2321` `cancel_work`, `:2336` `cancel_all_work`.
- No way to enumerate outstanding `WorkRef`s or check whether a given ref is still queued. Clients either track local state or subscribe to events.

### C6. `WorkSpec.content: String` is a capability regression
- `ids.rs:310` — `pub struct WorkSpec { pub content: String, pub origin: WorkOrigin }`.
- Rest of meerkat uses `meerkat_core::types::ContentInput` (multimodal). Work lane is text-only. Also no `HandlingMode`, no `RenderMetadata`, no deadline, no priority.

### C7. `realtime_attach`/`detach` are single-member only
- `handle.rs:2683`, `:2699` each take one `AgentIdentity`. A mob-wide "enable voice on these 5 members" is a loop with no atomicity guarantees. Add `realtime_attach_many(Vec<AgentIdentity>)` or an RPC that accepts a set.

### C8. No durable-voice-intent accessor
See B2. Mobkit needs to render "member X wants voice (may not be bound yet)" in the console. No public field exposes this. Either expose `voice_intent_present` or add a richer status enum that distinguishes "no intent" from "intent but binding pending."

### C9. No `agent_identity()` convenience on `MobMemberSnapshot`
- `handle.rs:89` — `impl MobMemberSnapshot` has only internal helpers. Every caller does `snapshot.agent_runtime_id.identity.clone()`. Trivial one-liner; easy fix; lots of ergonomic payoff across all consumers.

### C10. No `rotate_supervisor` RPC
- Rust method exists (`handle.rs:2186`). No entry in `rpc_catalog.rs`. If supervisor rotation is ever operator-triggered from outside the runtime process, this needs an RPC.

### C11. No structured `MobMemberListEntry` projection for fields that matter
Subsuming B1: define one canonical identity-first member shape and use it everywhere (list, snapshot, events). Today there are three (`MobMemberSnapshot`, `MobMemberListEntry`, `RosterEntry`) with overlapping-but-different fields.

### C12. No "replay from event_id" query for deterministic client-side state rebuild
`mob/events` exists (`rpc_catalog.rs:308`) but its signature wasn't fully verified — presumably streaming. A point-read `mob/events?since=<event_id>&limit=N` for cursor-based replay would let consumers reconcile cheaply after reconnect. Worth verifying whether it already does this.
