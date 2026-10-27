# H: Identity, live-session, WorkGraph, scheduling and integration designs

[Audit index](README.md) | [Coverage](coverage.md)

Original documentation and initial evidence line ranges refer to baseline `af82b6b3ab34faed9bf3e962d148d55f10dcd1dc`, unless an external dependency or historical revision is explicitly identified. Final-review citations refer to the corrected files in this change. Source excerpts may be de-indented or omit intervening lines; cited ranges identify the complete context. Quoted defects are preserved as evidence, not current usage guidance.

## H-001: The dual-plane table presents worker policy as a lack of mob identity and persistence

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/identity-first-doctrine.md:19-20`**

```text
| Key | generated runtime member id | stable `AgentIdentity` |
| Durability | none — dies with the process / idle-retire | continuity records, lease-fenced embodiment, resume-first restore |
```

Readers can mistake the allocation policy 'use the mob plane for ephemeral workers' for a storage guarantee, assume worker/member data cannot survive restart, or confuse the core member's AgentIdentity with a runtime binding ID.

**`(operator-retained path)/meerkat-mob-0.8.40/src/runtime/handle.rs:4732-4738`**

```text
pub identity: AgentIdentity,
```

The pinned core SpawnMemberSpec is addressed by AgentIdentity, not by an AgentRuntimeId-only primitive. MobKit's generated durable-incarnation alias is a composition choice, not the identity contract of every mob-plane member.

**`meerkat-mobkit/src/rpc/mob_methods.rs:951-978`**

```text
spec = spec.with_resume_bridge_session_id(sid);
```

The raw member ensure path explicitly accepts a resume session before forwarding to MobHandle::ensure_member; the substrate is not intrinsically no-resume.

**`meerkat-mobkit/src/mob_handle_runtime.rs:8550-8553`**

```text
MobBuilder::for_resume(spec.storage)
```

A nonempty stored mob event log selects the upstream resume builder; lines 8593-8596 call builder.resume().await. Mob state does not necessarily die with the process.

**`(operator-retained path)/meerkat-mob-0.8.40/src/runtime/builder.rs:7085-7097`**

```text
let (all_events, definition) = storage.replay_with_created_definition().await?;
```

The dependency owning mob lifecycle replays persisted definition/event authority on resume, corroborating the local composition path.

### Independent adjudication

Confirmed as an inaccurate description of substrate capabilities, not as a rejection of D1. The dated doctrine intentionally assigns ephemeral work to the mob plane, and that policy must remain. However, the opening comparison calls the mob's key a generated runtime member ID and its durability 'none' without making this a usage-policy qualification. The exact pinned core accepts AgentIdentity in SpawnMemberSpec, the raw MobKit member path can request resume, and MobKit actually selects the upstream resume builder for a nonempty event log. These are distinct from MobKit identity-plane continuity records and do not justify moving worker churn to that plane. Baseline inspected: af82b6b3ab34faed9bf3e962d148d55f10dcd1dc.

**`docs/design/identity-first-doctrine.md:16-22`**

```text
| Durability | none — dies with the process / idle-retire | continuity records, lease-fenced embodiment, resume-first restore |
```

This is the unqualified technical comparison; the separate D1 policy follows it.

**`(operator-retained path)/meerkat-mob-0.8.40/src/runtime/handle.rs:4732-4758`**

```text
pub identity: AgentIdentity,
```

The pinned upstream spawn request names a domain identity, separately from its binding and fresh/resume/fork launch mode.

**`meerkat-mobkit/src/rpc/mob_methods.rs:951-978`**

```text
spec = spec.with_resume_bridge_session_id(sid);
```

This is executed before handle.ensure_member(spec); raw mob-plane member admission has a resume carrier.

**`meerkat-mobkit/src/mob_handle_runtime.rs:8550-8553`**

```text
MobBuilder::for_resume(spec.storage)
```

Stored mob authority is a supported composition path, not necessarily process-local ephemeral state.

**`meerkat-mobkit/src/mob_handle_runtime.rs:8592-8596`**

```text
let handle = builder.resume().await?;
```

The resume builder is actually invoked when the event log is nonempty.

**Required correction:** Keep D1 and the historical decision/deployment census intact. Qualify the table as the intended MobKit allocation of responsibilities. Describe the mob key as its member AgentIdentity/member identifier, with MobKit-generated incarnation aliases where applicable, not the per-runtime AgentRuntimeId. Replace the absolute lack-of-durability claim with ephemeral-worker usage policy and note that underlying mob/session storage and launch modes can persist/resume; identity-plane continuity, leases and roster reconciliation remain the prescribed durable-population abstraction. Do not imply every worker will automatically be resumed.

### Changes and final verification

**Changed:** `docs/design/identity-first-doctrine.md`.

Qualified the two-plane table as MobKit's intended allocation, distinguished member AgentIdentity/incarnation aliases from AgentRuntimeId, and replaced the absolute no-durability claim with ephemeral-worker policy. Explicitly retained substrate persistence/resume without promising automatic worker restoration. D1, D2, the dated deployment census, and the phase plan are unchanged.

**Validation:** PASS: inspected mob_handle_runtime.rs bootstrap's MobBuilder::for_resume and builder.resume paths and rpc/mob_methods.rs's with_resume_bridge_session_id before ensure_member. Read-only H-001 assertions confirm the revised identity/durability qualifiers. Byte comparisons against HEAD confirm the historical decision sections are unchanged.

**Final review: pass.** The final table now describes product policy rather than a substrate limitation. It correctly distinguishes the member AgentIdentity from AgentRuntimeId, qualifies generated aliases, and acknowledges persistence/resume without promising automatic worker restoration. D1, D2, the dated deployment census, and the phases remain byte-identical to the baseline.

**`docs/design/identity-first-doctrine.md:16-30`**

```text
The table describes MobKit's intended allocation of responsibilities, not the
storage limits of the underlying mob/session substrate.
```

The revised key/durability rows and the following explicit non-guarantee now separate policy from implementation.

**`(operator-retained path)/meerkat-mob-0.8.40/src/runtime/handle.rs:4732-4758`**

```text
pub identity: AgentIdentity,
```

The pinned SpawnMemberSpec uses AgentIdentity and exposes a launch_mode supporting fresh/resume/fork.

**`meerkat-mobkit/src/mob_handle_runtime.rs:8550-8553,8592-8596`**

```text
let handle = builder.resume().await?;
```

Nonempty event-log bootstrap uses the actual resume builder, rather than an always-ephemeral substrate.

**`meerkat-mobkit/src/rpc/mob_methods.rs:951-978`**

```text
spec = spec.with_resume_bridge_session_id(sid);
```

The member path can carry an explicit resume source; the doc no longer denies that capability.

## H-002: The role-migration section overstates its no-retry guarantee

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/identity-first-doctrine.md:129-132`**

```text
withdraw a declaration, and MobKit never retries a resume meerkat refused.
```

The absolute sentence misdescribes recovery behavior and can lead operators to misattribute a repair/retry during activation to an unauthorized role-migration fallback.

**`meerkat-mobkit/src/identity_first/bridge.rs:5131-5136`**

```text
Err(error) if is_member_already_exists_error(&error) => {
```

resume_session has a specific recovery arm after a rejected spawn/resume. It does not categorically return every Meerkat refusal.

**`meerkat-mobkit/src/identity_first/bridge.rs:5230-5240`**

```text
"resume_session hit a roster collision; retiring the stale member and retrying resume"
```

After custody and durable-source checks, this path deliberately repairs the colliding occupant.

**`meerkat-mobkit/src/identity_first/bridge.rs:5274-5275`**

```text
if let Err(error) = self.spawn_member_spec(spawn_spec).await {
```

The implementation actually resubmits the same resume specification after repair; this is not only an obsolete explanatory comment.

**`(operator-retained path)/meerkat-mob-0.8.40/src/build.rs:706-725`**

```text
return Err(MobError::MemberRoleMigrationRejected {
```

The narrower D3 guarantee remains valid: role-migration refusal is a distinct error, not the member-already-exists collision case.

### Independent adjudication

Confirmed only for the absolute no-retry wording. The D3 migration-authority guarantee itself is supported: the exact predecessor declaration is checked upstream, and the local collision classifier matches MemberAlreadyExists, not a role-migration refusal. Nevertheless resume_session explicitly resubmits the unchanged resume spec after a custody-checked collision repair. There is also a separately gated typed-Absent/confirmed-missing-row fresh-spawn exception, so the replacement must not introduce a new global no-retry/no-fallback assertion. No evidence here shows MobKit bypassing or widening role-migration authority.

**`docs/design/identity-first-doctrine.md:129-132`**

```text
withdraw a declaration, and MobKit never retries a resume meerkat refused.
```

The sentence is broader than the role-migration rule this section needs to state.

**`meerkat-mobkit/src/identity_first/bridge.rs:273-275`**

```text
matches!(error, meerkat_mob::MobError::MemberAlreadyExists(_))
```

The independently traced retry arm has a narrow collision predicate, not an arbitrary migration-refusal match.

**`meerkat-mobkit/src/identity_first/bridge.rs:5230-5275`**

```text
if let Err(error) = self.spawn_member_spec(spawn_spec).await {
```

After collision custody, durable-source and retirement checks, the existing resume spec is submitted again. This is executable retry behavior, not only a log message.

**`meerkat-mobkit/src/identity_first/bridge.rs:5321-5358`**

```text
if durable_snapshot_is_typed_absent(&error)
```

A distinct branch additionally requires durable_session_row_is_absent before creating a fresh successor. It must not be accidentally contradicted by a new global fallback prohibition.

**`(operator-retained path)/meerkat-mob-0.8.40/src/build.rs:704-725`**

```text
if declared_predecessor_role.as_str() != stored_role {
```

The exact predecessor mismatch returns MemberRoleMigrationRejected. Narrowing the prose does not relax that refusal.

**Required correction:** Replace the absolute final clause with a migration-specific guarantee: MobKit never infers or widens migration authority and does not recover from a role-migration refusal by altering the declaration or falling back to a fresh spawn. If recovery is discussed, distinguish the custody- and durable-source-checked MemberAlreadyExists retry of the same resume spec. Do not rewrite this decision as a blanket ban on all retries or on the separately typed never-persisted fallback.

### Changes and final verification

**Changed:** `docs/design/identity-first-doctrine.md`.

Narrowed the no-recovery guarantee to role-migration refusals: declarations are not inferred/widened/altered and those refusals do not cause a fresh-spawn fallback. Distinguished the custody- and durable-source-checked MemberAlreadyExists retry of an unchanged resume specification without imposing a blanket no-retry/no-fallback rule.

**Validation:** PASS: inspected identity_first/bridge.rs collision repair and subsequent spawn_member_spec(spawn_spec), plus its distinct durable_snapshot_is_typed_absent/durable_session_row_is_absent fallback. H-002 assertions confirm the migration-specific wording and removal of the absolute no-retry claim.

**Final review: pass.** The no-recovery assertion is now confined to altering migration authority or fresh-spawning after a role-migration refusal. The distinct MemberAlreadyExists repair is accurately described as retrying the same resume specification after custody/source checks. The text does not forbid every retry or the separately typed absent-source fallback.

**`docs/design/identity-first-doctrine.md:137-144`**

```text
does not recover from a role-migration refusal by altering the declaration or
falling back to a fresh spawn.
```

The next sentence explicitly permits the unchanged-spec collision retry without widening migration authority.

**`meerkat-mobkit/src/identity_first/bridge.rs:273-275,5230-5275`**

```text
matches!(error, meerkat_mob::MobError::MemberAlreadyExists(_))
```

Only the typed collision selects this repair. The implementation checks the durable source before retirement and calls spawn_member_spec(spawn_spec) again.

**`meerkat-mobkit/src/identity_first/bridge.rs:5321-5355`**

```text
if durable_snapshot_is_typed_absent(&error)
                    && self.durable_session_row_is_absent(session_id).await
```

A separate, narrowly gated fresh-spawn fallback still exists; the revised prose does not globally prohibit it.

**`(operator-retained path)/meerkat-mob-0.8.40/src/build.rs:706-725`**

```text
if declared_predecessor_role.as_str() != stored_role {
```

The pinned upstream owner rejects an inexact declared predecessor and returns before examining an inert declaration when roles already match.

## H-003: Live integration still declares a necessary development pin after the registry repin

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/live-sessions.md:3-5`**

```text
Status: console HTTP integration uses the Meerkat 0.8.39 / MobKit 0.8.36
baseline with the reviewed upstream Live owner-seam development pin recorded
in `Cargo.lock`. Development bindings are not release pins.
```

An integrator is told the released dependency still lacks required owner seams and may restore obsolete Git patches or disable a configured feature unnecessarily. The current-status banner also obscures which exact dependency authority was actually audited.

**`Cargo.toml:8-11`**

```text
version = "0.8.39"
```

The audited workspace is MobKit 0.8.39, not the stated current 0.8.36 baseline.

**`Cargo.lock:2205-2208`**

```text
name = "meerkat"
version = "0.8.40"
source = "registry+https://github.com/rust-lang/crates.io-index"
```

The facade is resolved from crates.io 0.8.40. Read-only TOML inspection found the same 0.8.40 registry source for the checked core, mob, live, schedule, and WorkGraph dependencies; the described Git development pin is not present.

**`meerkat-mobkit/Cargo.toml:77-81`**

```text
meerkat-live = { version = "=0.8.40" }
```

The exact manifest pin agrees with the lockfile rather than relying on current upstream branch documentation.

**`meerkat-mobkit/src/console_voice/live_host.rs:185-218`**

```text
PublicGptLivePlaybackPolicy::ProviderManagedUnmeasured,
```

Current composition consumes the supposedly development-only owner seam, along with LiveContextSummaryPolicy and Concurrent mode, from the registry dependency. ExistingMember is selected in live_wiring.rs:2398-2412.

**`.github/workflows/release.yml:390-393`**

```text
--features openai-live --locked --release --target "${{ matrix.target }}"
```

The release build explicitly enables the public feature; the broad later statement at live-sessions.md:670-671 that stock crates.io builds advertise no such atoms is no longer an accurate unconditional build distinction.

### Independent adjudication

Confirmed as stale present-status/rebinding advice, not as a false historical report of the 0.8.38/0.8.39 gaps or paid-test evidence. Independent TOML inspection found MobKit 0.8.39 and registry-sourced Meerkat/core/mob/live/schedule/workgraph 0.8.40; the current host consumes Concurrent summary and unmeasured playback APIs directly and selects ExistingMember. The July/September chronology may stay, but the top status and instruction that a development pin remains necessary do not describe this checkout. Capability advertisement is controlled by compilation plus actual registered/composed authority, not by whether Cargo fetched a registry or Git source. This audit does not establish new real-provider acceptance or registry-release success.

**`docs/design/live-sessions.md:3-5`**

```text
baseline with the reviewed upstream Live owner-seam development pin recorded
```

The page presents this as its status, not solely as dated historical evidence.

**`Cargo.toml:8-11`**

```text
version = "0.8.39"
```

The workspace release-line authority differs from the claimed current MobKit baseline.

**`Cargo.lock:2204-2208`**

```text
name = "meerkat"
version = "0.8.40"
source = "registry+https://github.com/rust-lang/crates.io-index"
```

The resolved facade is a registry dependency, not the described Git development patch. The other checked Meerkat family entries have the same version/source.

**`meerkat-mobkit/src/console_voice/live_host.rs:189-218`**

```text
.with_bootstrap_mode(LiveContextBootstrapMode::Concurrent);
```

Current production composition uses the summary-owner seam and ProviderManagedUnmeasured policy from the pinned dependency.

**`meerkat-mobkit/src/live_wiring.rs:2398-2412`**

```text
meerkat_mob_mcp::live_delegation::LiveDelegationExecutionPolicy::ExistingMember,
```

The console-specific handler selects the existing-member execution policy, corroborating the available owner API.

**`meerkat-mobkit/src/live_wiring.rs:2148-2188`**

```text
let Some(configured) = &self.configured else {
```

The feature-enabled capability path still requires configuration, composed phase authority, a live host and a bound-ready binder; an enabled build alone is not availability.

**`.github/workflows/release.yml:391-393`**

```text
--features openai-live --locked --release --target "${{ matrix.target }}"
```

The current release build explicitly enables public Live support against the locked dependency selection.

**`meerkat-mobkit/src/http_console.rs:571-580`**

```text
"readiness_method": crate::console_voice::VOICE_READINESS_METHOD
```

HTTP console discovery remains configured/authenticated target-readiness discovery, not a reason to advertise generic strict Live atoms or microphone availability unconditionally.

**Required correction:** Set the page's current checkout baseline to MobKit 0.8.39 with exact crates.io Meerkat 0.8.40 pins. Explicitly mark the 0.8.38 gap survey and September development-pin/rebinding narrative as historical; remove the current requirement to retain a Git patch and do not instruct a repin to 0.8.39. Preserve old failed/unqualified audio evidence as history, not newly passing acceptance. Qualify the crates.io-versus-development capability sentence by openai-live plus explicit composed host registration. Preserve unconfigured unavailability and authenticated per-target readiness. Do not imply that HTTP console's generic feature_capabilities array is populated: its current discovery path is voice.readiness_method.

### Changes and final verification

**Changed:** `docs/design/live-sessions.md`.

Updated the audited baseline to MobKit 0.8.39 and exact registry Meerkat 0.8.40. Marked the 0.8.38 gap survey, September development pins, and older compatibility limits as historical; removed current instructions to retain a Git patch or return to 0.8.39. Preserved lineage and qualification warnings without claiming new audio acceptance. Capability wording now depends on openai-live and composed strict registration, while console HTTP discovery remains authenticated target readiness, not generic feature atoms.

**Validation:** PASS: tomllib verified workspace version 0.8.39, all 18 exact Meerkat manifest pins at =0.8.40 across dependency tables, and 19 matching registry lockfile entries including transitive WorkGraph. Inspected console_voice/live_host.rs Concurrent summary and ProviderManagedUnmeasured composition; reviewed current registration/readiness evidence from adjudication. H-003 assertions pass.

**Final review: pass.** Current checkout provenance is corrected to MobKit 0.8.39 and registry Meerkat 0.8.40. The older gap survey and development-pin account are explicitly historical, including the previously necessary Git pin. No new paid/browser acceptance is claimed. Feature compilation, composed strict-host registration, and authenticated HTTP target readiness are distinguished correctly.

**`docs/design/live-sessions.md:3-10,49-52,432-468,713-720`**

```text
The registry repin alone does not establish new real-provider/browser acceptance.
```

The current status removes the patch requirement while retaining historical failures and future qualification gates.

**`Cargo.toml:8-11`**

```text
version = "0.8.39"
```

The workspace release version agrees with the revised baseline.

**`Cargo.lock:2204-2208`**

```text
name = "meerkat"
version = "0.8.40"
source = "registry+https://github.com/rust-lang/crates.io-index"
```

Independent TOML inspection also verified all 25 versioned declarations across both manifests and all 35 resolved Meerkat-family registry packages, not merely the facade.

**`meerkat-mobkit/src/console_voice/live_host.rs:189-218`**

```text
PublicGptLivePlaybackPolicy::ProviderManagedUnmeasured,
```

The checked-in host directly composes the released unmeasured policy and Concurrent LiveContextSummaryPolicy owner seams.

**`meerkat-mobkit/src/live_wiring.rs:2148-2197,2398-2412`**

```text
if !configured.phase_authority_composed {
                return Vec::new();
            }
```

Capability publication depends on feature/configured/composed owner state, not Git versus registry origin; the console separately selects ExistingMember.

**`meerkat-mobkit/src/http_console.rs:571-581,5183-5185,5687-5689`**

```text
"readiness_method": crate::console_voice::VOICE_READINESS_METHOD
```

HTTP experience publishes target-readiness discovery only for a configured authenticated writable console; its generic feature_capabilities arrays remain empty.

## H-004: Voice cancellation fences are documented as permanent but closed slots are evicted

**Severity:** high. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/live-sessions.md:234-236`**

```text
- Cancellation tombstones are retained for the process lifetime. A bounded
  4,096-entry registry refuses new request IDs rather than evicting a fence and
  allowing a delayed request to recreate closed work.
```

Clients relying on the documented process-lifetime idempotency/cancellation guarantee can replay an old request after eviction and create new voice work. Capacity planning is also wrong: closed requests yield capacity rather than inevitably exhausting a never-evicted registry.

**`meerkat-mobkit/src/console_voice.rs:74-80`**

```text
const CLOSED_RETENTION: Duration = Duration::from_mins(10);
```

Closed-slot retention is time-bounded; the same constants define MAX_REQUESTS=4096 and MAX_CLOSED_PER_PRINCIPAL=32.

**`meerkat-mobkit/src/console_voice.rs:261-264`**

```text
if now.saturating_duration_since(retired_at) >= CLOSED_RETENTION {
            requests.remove(&key);
```

The reaper removes expired cancellation/closed records, contrary to process-lifetime retention.

**`meerkat-mobkit/src/console_voice.rs:271-285`**

```text
if *kept >= MAX_CLOSED_PER_PRINCIPAL {
            requests.remove(key);
```

The per-principal bound reaps older closed entries even before time expiry; the global-capacity pass also removes oldest closed slots before refusing. Opening/active/failing-to-close slots are excluded from reaping.

**`meerkat-mobkit/src/console_voice.rs:1850-1870`**

```text
assert_eq!(requests.len(), 0, "expired closed slots are reaped");
```

The checked-in regression test explicitly pins expiry and capacity recovery, so the difference is intentional current behavior, not merely unused constants.

**`meerkat-mobkit/src/console_voice.rs:794-815`**

```text
reap_closed_requests(&mut requests, tokio::time::Instant::now()).await;
```

The production new-open path invokes reaping and treats a missing request key as a new slot. The API therefore cannot promise permanent request-ID rejection after a fence has been removed.

### Independent adjudication

Confirmed directly from both production insertion paths and the reaper, with an important narrowing of the proposed timing language. Closed slots are not permanent. Reaping is lazy: inserting a previously unseen request ID invokes it; simply retrying a still-retained existing ID does not first expire that slot. Ordinary closed slots receive retired_at when the reaper first observes them, while close-before-open tombstones set it immediately. Expiry and per-principal/global reclamation remove only closed entries. Thus ten minutes is an eligibility threshold from the recorded observation time, not a guaranteed background deletion timer or a guaranteed minimum retention under capacity pressure. No runtime behavior change is warranted by this documentation audit.

**`docs/design/live-sessions.md:234-236`**

```text
Cancellation tombstones are retained for the process lifetime.
```

The page also says close permanently fences a request ID; both absolute guarantees conflict with current storage.

**`meerkat-mobkit/src/console_voice.rs:74-82`**

```text
const CLOSED_RETENTION: Duration = Duration::from_mins(10);
```

The same constants set 4096 total requests and 32 closed slots per principal.

**`meerkat-mobkit/src/console_voice.rs:252-285`**

```text
let retired_at = *state.retired_at.get_or_insert(now);
```

The reaper skips nonclosed slots, timestamps its first closed observation, removes entries at the retention threshold, then removes oldest closed slots for the per-principal and total-capacity bounds.

**`meerkat-mobkit/src/console_voice.rs:794-820`**

```text
reap_closed_requests(&mut requests, tokio::time::Instant::now()).await;
```

The missing-key branch of open reclaims closed records before capacity checking and insertion. Once an old key was removed by such a pass, a later open is a new request.

**`meerkat-mobkit/src/console_voice.rs:915-950`**

```text
retired_at: Some(tokio::time::Instant::now()),
```

Close-before-open creates a timestamped tombstone, then reaps again after insertion; foreign/prior closed IDs cannot consume unbounded permanent capacity.

**`meerkat-mobkit/src/console_voice.rs:1850-1870`**

```text
assert_eq!(requests.len(), 0, "expired closed slots are reaped");
```

The checked-in regression explicitly expects expiry; this is source evidence, not a test newly executed in this audit.

**Required correction:** Remove 'permanently' and the process-lifetime/no-eviction promise. State that duplicate-open and cancellation fences hold while their closed slot is retained; closed slots are lazily reaped on new-ID admission, are eligible after ten minutes from the registry's recorded closed-observation time, and can be reclaimed sooner to retain the newest 32 closed slots per principal or to free the 4096-entry total registry. Live/opening/failed-cleanup slots are not evicted by this reaper. Clients must never reuse a closed request ID. Preserve retrying the exact in-progress close after the ten-second observation timeout; do not promise deletion precisely ten minutes after close.

### Changes and final verification

**Changed:** `docs/design/live-sessions.md`.

Replaced permanent/process-lifetime voice fencing with retention-scoped duplicate/cancellation guarantees. Documented lazy new-ID reaping, ten-minute eligibility from recorded closed observation, immediate observation for close-before-open tombstones, earlier 32-per-principal/4096-total reclamation, and exclusion of opening/active/failed-cleanup slots. Prohibited reuse of closed request IDs while preserving exact in-progress close retries after the ten-second observation timeout.

**Validation:** PASS: read console_voice.rs constants, reap_closed_requests, existing-key versus new-key open admission, and close-before-open insertion/reaping. H-004 assertions confirm removal of both permanent guarantees and presence of timing, capacity, non-reuse, and lazy-reaping boundaries.

**Final review: pass.** Permanent fencing was removed in both places. The replacement accurately describes retention-scoped duplicate/cancellation protection, lazy new-ID reaping, recorded-observation timing, earlier capacity reclamation, and the non-eviction of live/failed-cleanup slots. It preserves retrying an in-progress close while prohibiting reuse of a closed ID.

**`docs/design/live-sessions.md:229-255`**

```text
Duplicate-open and cancellation guarantees last only while the slot is
  retained.
```

The surrounding bullets include both the ten-minute eligibility qualification and the 32/4096 reclamation bounds.

**`meerkat-mobkit/src/console_voice.rs:74-81,243-286`**

```text
let retired_at = *state.retired_at.get_or_insert(now);
```

The reaper observes only closed slots, expires by the recorded instant, then enforces per-principal and global bounds; no background expiry guarantee exists.

**`meerkat-mobkit/src/console_voice.rs:795-807,916-943`**

```text
reap_closed_requests(&mut requests, tokio::time::Instant::now()).await;
```

Existing-key lookup bypasses reaping. New open/close IDs invoke it; close-before-open records retired_at immediately.

**`meerkat-mobkit/src/console_voice.rs:153-156,947-950`**

```text
"Voice teardown is still pending; retry the same request",
```

The ten-second close observation timeout maps to Busy/voice_busy rather than successful closure, preserving the exact retry instruction.

## H-005: The ordinary Live RPC design incorrectly promises the full method family on console HTTP

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/live-sessions.md:599-604`**

```text
5. **RPC surface** (unified stdin + console): `mobkit/live/open`,
   `mobkit/live/status`, `mobkit/live/close`, `mobkit/live/refresh`,
   `mobkit/live/send_input`, `mobkit/live/commit_input`,
   `mobkit/live/interrupt`, `mobkit/live/truncate`. Params accept an
   IDENTITY TARGET —
   `{identity: "device"}` or `{member_id}` or raw `{session_id}` —
```

A browser/LAN integrator following this section calls absent HTTP methods or supplies unsupported target forms. It also conflicts with this document's otherwise-correct request-fenced console voice boundary.

**`meerkat-mobkit/src/rpc.rs:4913-4919`**

```text
None => crate::live_wiring::live_unavailable_response(response_id),
```

The unified stdin dispatcher has the generic mobkit/live/* handler and typed unconfigured response described in the design.

**`meerkat-mobkit/src/http_console.rs:638-656`**

```text
) || crate::console_voice::is_channel_method(&request_method)
```

HTTP interception is specifically the seven console_voice request methods plus the bounded console channel subset, not the generic stdin handler.

**`meerkat-mobkit/src/console_voice.rs:54-65`**

```text
"mobkit/live/playback_owner/register"
            | "mobkit/live/playback_owner/revoke"
            | "mobkit/live/status"
            | "mobkit/live/close"
            | "mobkit/live/refresh"
            | "mobkit/live/interrupt"
            | "live/webrtc/answer"
```

The exact HTTP voice channel allowlist excludes raw open, send_input, commit_input, truncate, and playback_complete. HTTP voice opens through mobkit/console/voice/open instead.

**`meerkat-mobkit/src/http_console.rs:8931-8939`**

```text
code: -32601,
                message: "Method not found".to_string(),
```

The ordinary HTTP RPC match has no generic Live fallback, so omitted methods reach method-not-found rather than the advertised ordinary Live path.

**`meerkat-mobkit/src/console_voice.rs:714-718`**

```text
.get("identity")
```

The console channel controller requires the exact identity and channel of an owned request (principal/channel checks continue through lines 723-740); the ordinary member_id/session_id target vocabulary is not an interchangeable HTTP authorization path.

### Independent adjudication

Confirmed as a missing implemented-versus-original-design boundary, not proof that the original proposal had to ship unchanged. The July design paragraph is still unqualified 'unified stdin + console' guidance inside a living page whose earlier section documents the actual HTTP fence. Independently tracing both dispatchers shows generic Live routing on stdin, but HTTP intercepts only console request verbs and is_channel_method's exact seven-name control subset. Raw open/input/commit/truncate/playback_complete have no generic HTTP fallback. Merely appearing in read-only/access classifiers does not mount a method. The safe fix can retain the original design while explicitly saying its full-console proposal is not the implemented HTTP contract.

**`docs/design/live-sessions.md:599-610`**

```text
5. **RPC surface** (unified stdin + console): `mobkit/live/open`,
```

The claimed surface and interchangeable target forms are not qualified as superseded here.

**`meerkat-mobkit/src/rpc.rs:4913-4921`**

```text
if method.starts_with("mobkit/live/")
```

The unified stdin dispatcher has the general family and returns live_unavailable when no handler is installed.

**`meerkat-mobkit/src/http_console.rs:638-657`**

```text
) || crate::console_voice::is_channel_method(&request_method)
```

HTTP diverts only its explicit request-fenced voice family and the bounded channel allowlist before normal console dispatch.

**`meerkat-mobkit/src/console_voice.rs:54-65`**

```text
"mobkit/live/playback_owner/register"
            | "mobkit/live/playback_owner/revoke"
            | "mobkit/live/status"
            | "mobkit/live/close"
            | "mobkit/live/refresh"
            | "mobkit/live/interrupt"
            | "live/webrtc/answer"
```

The complete HTTP channel subset excludes the generic raw open, input, commit, truncate and playback-complete methods.

**`meerkat-mobkit/src/console_voice.rs:714-740`**

```text
.filter(|((owner, _), slot)| owner == principal && slot.identity == identity)
```

HTTP channel dispatch reads identity and channel_id and checks the authenticated principal's uncancelled owned session. member_id/session_id are not interchangeable authorization targets here.

**`meerkat-mobkit/src/http_console.rs:8931-8939`**

```text
code: -32601,
```

The normal console dispatcher ends in method-not-found, not a generic Live forwarding arm.

**Required correction:** Label the full ordinary Live family and identity/member_id/session_id target alternatives as unified stdin behavior. Either update this paragraph or retain it as the original proposal with an explicit superseded-console qualification and a link to HTTP request fencing. Name the implemented HTTP open as mobkit/console/voice/open and enumerate its request-owned channel subset. State that raw open/send_input/commit_input/truncate/playback_complete are not HTTP fallbacks. Do not remove the original transport history or claim all Live methods are absent from HTTP.

### Changes and final verification

**Changed:** `docs/design/live-sessions.md`.

Scoped the ordinary full Live family and identity/member_id/session_id alternatives to unified stdin. Marked the original full-console proposal as superseded, retained its design history, and linked to request-fenced HTTP open. Enumerated all seven current HTTP channel methods and explicitly excluded raw open/send_input/commit_input/truncate/playback_complete fallbacks. Clarified strict stdin capability discovery versus console readiness and measured-host observations versus the unmeasured console.

**Validation:** PASS: extracted all seven methods from console_voice.rs::is_channel_method and asserted each appears in the HTTP boundary text. Checked authenticated identity/channel ownership evidence and dispatcher distinctions from the adjudication. H-005 assertions confirm the stdin label, superseded-console boundary, and HTTP exclusions.

**Final review: pass.** The full ordinary family and alternative target spellings are now explicitly stdin-only. The superseded full-console proposal is linked to the actual request-fenced HTTP contract. The seven channel methods exactly match the implementation; raw open/input/commit/truncate/playback_complete are not advertised as HTTP fallbacks. The strict stdin table and measured-host discussion are separated from unmeasured console behavior.

**`docs/design/live-sessions.md:257-267,359-396,638-655`**

```text
The ordinary `identity`/`member_id`/`session_id` alternatives are not
   interchangeable HTTP authorization targets.
```

The ordinary section is labelled unified stdin and points to the exact HTTP method subset.

**`meerkat-mobkit/src/console_voice.rs:54-65,715-741`**

```text
"mobkit/live/playback_owner/register"
            | "mobkit/live/playback_owner/revoke"
            | "mobkit/live/status"
            | "mobkit/live/close"
            | "mobkit/live/refresh"
            | "mobkit/live/interrupt"
            | "live/webrtc/answer"
```

All seven names match the revised list. Dispatch separately requires exact identity/channel under the authenticated principal's retained request.

**`meerkat-mobkit/src/http_console.rs:638-657,8931-8939`**

```text
) || crate::console_voice::is_channel_method(&request_method)
```

HTTP explicitly intercepts this subset; the console dispatcher does not mount a generic Live fallback and unmatched methods reach -32601.

**`meerkat-mobkit/src/rpc.rs:4913-4921`**

```text
if method.starts_with("mobkit/live/")
```

The unified stdin dispatcher, unlike HTTP, routes the general Live namespace.

## H-006: Caller-field rejection is incorrectly applied to every live/open rather than strict execution profiles

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/live-sessions.md:742-744`**

```text
- **Host-trusted voice profiles**: `mobkit/live/open` rejects caller-supplied
  instructions, mode, model, provider, tools, Responses configuration, and
  capability claims.
```

Ordinary Live clients are told supported channel-scoped controls are rejected, directly contradicting the document's earlier ordinary model/provider override instructions.

**`meerkat-mobkit/src/live_wiring.rs:4645-4669`**

```text
if let Some(execution_identity) = parsed.execution_identity.as_ref() {
```

The explicit rejection of model/provider and instructions is inside the strict execution_identity branch, not unconditional for all Live opens.

**`meerkat-mobkit/src/live_wiring.rs:4809-4820`**

```text
let turning_mode = parsed
        .turning_mode
        .unwrap_or(RealtimeTurningMode::ProviderManaged);
```

The ordinary path honors the caller's turning mode, even in an openai-live-enabled build.

**`meerkat-mobkit/src/live_wiring.rs:4841-4852`**

```text
projection.append_ephemeral_system_overlay(overlay);
```

Ordinary open accepts caller instructions as a channel-only overlay rather than rejecting them.

**`meerkat-mobkit/src/live_wiring.rs:4853-4857`**

```text
apply_live_open_identity_selection(
        &mut projection.open_config.llm_identity,
        provider_override,
        parsed.model,
    );
```

The ordinary path applies caller model/provider selection; provider requires an explicit model and strict provider parsing, without changing the durable member identity.

### Independent adjudication

Confirmed as an overbroad current scope statement, with the historical heading retained. I checked both the strict branch and the ordinary fallthrough rather than treating a strict-profile validation function as the whole API. execution_identity conflicts with model/provider/instructions; absent that selection, ordinary open honors turning_mode, builds an ephemeral instruction overlay, and applies channel-local model/provider selection. Git blame further shows this 'Host-trusted voice profiles' bullet was rewritten on 2026-08-28 beneath the older 0.7.32 heading, so the heading does not make it a frozen accurate record of all 0.7.32 opens. The valid strict-profile boundary must not be weakened.

**`docs/design/live-sessions.md:742-747`**

```text
- **Host-trusted voice profiles**: `mobkit/live/open` rejects caller-supplied
```

The bullet omits the execution_identity/strict-profile condition even though this page also teaches ordinary model/provider overrides.

**`meerkat-mobkit/src/live_contracts.rs:346-368`**

```text
let Some(raw) = object.get("execution_identity") else {
```

No execution_identity returns None; legacy model/provider conflicts are checked only after this optional strict selection exists.

**`meerkat-mobkit/src/live_wiring.rs:4653-4669`**

```text
if let Some(execution_identity) = parsed.execution_identity.as_ref() {
```

The subsequent model/provider/instruction refusal belongs to the strict branch.

**`meerkat-mobkit/src/live_wiring.rs:4798-4811`**

```text
if provider_override.is_some() && parsed.model.is_none() {
```

The ordinary path validates provider selection and requires an explicit paired model rather than universally forbidding either field.

**`meerkat-mobkit/src/live_wiring.rs:4812-4814`**

```text
.unwrap_or(RealtimeTurningMode::ProviderManaged);
```

The caller's optional ordinary turning_mode is used, with ProviderManaged only the default.

**`meerkat-mobkit/src/live_wiring.rs:4841-4857`**

```text
projection.append_ephemeral_system_overlay(overlay);
```

Ordinary instructions are a channel-only overlay; the following apply_live_open_identity_selection also applies provider/model to that projection.

**Required correction:** Qualify this rejection bullet as applying to strict public/experimental opens selecting execution_identity and a host-registered profile. Contrast ordinary stdin compatibility opens, which support channel-scoped instructions, model, a validated provider paired with model, and turning_mode. Do not call the latter 'mode', promise arbitrary tools/Responses/capability controls on the ordinary path, or imply console voice accepts these caller overrides. Preserve the dated feature lineage.

### Changes and final verification

**Changed:** `docs/design/live-sessions.md`.

Limited caller-field rejection to strict public/experimental opens selecting execution_identity and a host-registered profile. Contrasted ordinary stdin channel-scoped instructions, model, validated provider paired with model, and turning_mode, without extending those controls to console voice or promising arbitrary tool/Responses/capability controls.

**Validation:** PASS: inspected live_wiring.rs's execution_identity rejection branch and ordinary fallthrough honoring turning_mode, append_ephemeral_system_overlay, and apply_live_open_identity_selection with provider/model validation. H-006 assertions confirm both scopes are explicit.

**Changed:** `docs/design/live-sessions.md`.

Separated strict nested execution_identity unknown-field rejection from ignored extra top-level capabilities/feature_capabilities fields. Stated that ignored fields cannot grant authority and capabilities remain host-owned. Preserved strict caller instruction/model/provider/tool/Responses restrictions, host-profile authority, ordinary stdin channel-scoped controls and provider/model pairing, and the console-voice exclusion.

**Validation:** Read meerkat-mobkit/src/live_contracts.rs:124-138,345-410 and src/live_wiring.rs:1544-1582,4698-4706. LiveExecutionIdentityV1 denies unknown nested fields; the top-level strict denylist does not include the capability properties; GatewayLiveOpenParams tolerates unknown fields, and only typed parsed values reach the shared host. Source/document assertions, balanced-fence checks, conflict-marker checks, and git diff --check passed.

**Final review: pass.** Independently re-reviewed after fixes-review-residuals.json. The final text now expressly distinguishes unknown fields rejected inside the nested execution_identity profile-selection envelope from ignored extra top-level capabilities/feature_capabilities fields, and states that ignored fields confer no authority. This matches the separate strict nested deserializer, explicit top-level denylist, tolerant GatewayLiveOpenParams, and typed shared-host call. Strict instructions/model/provider/tool/Responses restrictions, ordinary stdin instructions/model/paired-provider/turning_mode controls, and the console exclusion remain intact. H-R001 is resolved without a runtime change.

**`docs/design/live-sessions.md:791-805`**

```text
Extra top-level capability fields such as `capabilities` or
  `feature_capabilities` are ignored, not rejected; they cannot grant authority,
  and capabilities remain host-owned.
```

The immediately preceding sentence confines unknown-field rejection to the nested execution_identity envelope. The following sentences preserve the strict-versus-ordinary and stdin-versus-console distinctions.

**`meerkat-mobkit/src/live_contracts.rs:124-138,345-367,374-410`**

```text
#[serde(deny_unknown_fields)]
pub struct LiveExecutionIdentityV1 {
```

Unknown fields are rejected inside execution_identity, whose fields are version/profile_id. The separate top-level strict denylist enumerates instructions/tools/Responses/mode fields, but neither capabilities nor feature_capabilities.

**`meerkat-mobkit/src/live_wiring.rs:1544-1582`**

```text
/// before `handle_live_method`; unknown fields are ignored here so the
/// target spellings pass through untouched.
```

GatewayLiveOpenParams uses ordinary Deserialize without deny_unknown_fields and has no capability-claim field; extra top-level capability properties are discarded.

**`meerkat-mobkit/src/rpc.rs:5703-5720`**

```text
Ok(Some(_)) if capability_available => None,
```

After nested parsing and the explicit surface denylist, an otherwise valid configured strict request proceeds. There is no additional top-level unknown-field rejection here.

**`meerkat-mobkit/src/live_wiring.rs:4698-4706,4800-4857`**

```text
projection.append_ephemeral_system_overlay(overlay);
```

The strict host receives only typed selected fields, while the ordinary path genuinely supports the documented instructions/model/paired-provider/turning_mode controls. Ignored capability properties do not reach the shared owner as authority.

## H-007: The obsolete oldest-message seed clamp is still described as the active implementation

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/live-sessions.md:748-755`**

```text
- **Seed clamp (upstream ask 30 STOPGAP)**: providers cap live instructions
  at 65,536 tokens, so long member transcripts overflow the projected seed
  at open. `runtime_options.live.seed_max_chars` (object form) sets a
  gateway-wide serialized-char budget; per-open `seed_max_chars` overrides
  it. Whole canonical conversation messages drop oldest-first. System and
  SystemNotice authority text is excluded from experimental provider
  commentary entirely. Remove the clamp when Meerkat ships a machine-owned
  seed-window projection (ask 30).
```

Clients reason about the wrong retained context and maintainers are instructed to wait for, or implement removal upon, an upstream seam that already shipped. The generic provider token-cap wording also obscures that seed_max_chars is a serialized-character window, not a guaranteed universal provider token budget.

**`meerkat-mobkit/src/live_wiring.rs:3794-3799`**

```text
// 0.7.28: seed bounding is upstream-owned (`live/open.seed_max_chars`,
    // upstream ask 30 SHIPPED) — the windowed projection preserves the
    // enabled root context, an affordable compaction summary, and the
    // identity/tombstone/rewrite-generation/canonical-image sidecars, and
    // reports degraded continuity explicitly. This replaced mobkit's
    // oldest-first clamp stopgap.
```

The retained ordinary compatibility implementation explicitly identifies the old clamp as replaced.

**`meerkat-mobkit/src/live_wiring.rs:3800-3814`**

```text
meerkat::session_runtime::live_orchestration::realtime_projection_messages_with_window(
```

Production calls the dependency's seed-window projection, not a MobKit oldest-message loop.

**`meerkat-mobkit/src/live_wiring.rs:4819-4837`**

```text
.prepare_open_projection(session_id, turning_mode, seed_window)
```

The openai-live ordinary branch also forwards the window into shared upstream projection authority.

**`(operator-retained path)/meerkat-0.8.40/src/session_runtime/live_orchestration.rs:584-587`**

```text
/// Select a bounded, deterministic projection. Existing typed compaction
/// summary content is the optional head; the tail is retained only at complete
/// conversational-turn boundaries, with contiguous System, SystemNotice, and
/// injected-context rows glued to the user message they precede.
```

The pinned implementation at lines 605-682 selects an affordable summary and a contiguous suffix of complete turns and returns Windowed status. This differs materially from dropping individual oldest messages.

### Independent adjudication

Confirmed only as stale current/removal guidance; an oldest-first clamp existing historically is not disputed. Both current cfg branches use the upstream seed-window authority, and I inspected the exact 0.8.40 algorithm: full projection when affordable; otherwise an affordable latest typed compaction summary plus a contiguous suffix of complete conversational turns, with adjacent system/notices/injected context grouped into those turns, and an explicit Windowed result. Local immutable commit 775ca6472fd962beaff75fa6b915c33ced977242 already integrated the upstream window on 2026-07-11, so the later 'remove when it ships' instruction is obsolete. Do not replace the historical section with claims that a seed window generates a fresh current-context summary or guarantees a universal provider token limit.

**`docs/design/live-sessions.md:748-755`**

```text
Remove the clamp when Meerkat ships a machine-owned
```

The versioned feature-history paragraph still gives a pending action rather than marking its stopgap replaced.

**`meerkat-mobkit/src/live_wiring.rs:3794-3814`**

```text
meerkat::session_runtime::live_orchestration::realtime_projection_messages_with_window(
```

The ordinary compatibility implementation invokes upstream selection; the surrounding code explicitly calls the MobKit stopgap replaced.

**`meerkat-mobkit/src/live_wiring.rs:4819-4837`**

```text
let seed_window = match parsed.seed_max_chars.or(ctx.seed_max_chars) {
```

The openai-live ordinary path preserves per-open-over-gateway precedence, constructs LiveSeedWindow and passes it to prepare_open_projection.

**`(operator-retained path)/meerkat-0.8.40/src/session_runtime/live_orchestration.rs:584-598`**

```text
/// summary content is the optional head; the tail is retained only at complete
```

The pinned API contract describes complete-turn selection, not individual oldest-message removal.

**`(operator-retained path)/meerkat-0.8.40/src/session_runtime/live_orchestration.rs:608-680`**

```text
status: LiveSeedProjectionStatus::Windowed {
```

The actual algorithm budgets a typed summary, finds turn boundaries, retains an affordable contiguous suffix and reports dropped_messages/included_compaction_summary.

**`meerkat-mobkit/src/console_voice/live_host.rs:204-218`**

```text
None => LiveContextSummaryPolicy::new(
```

Fresh summary production and Concurrent console bootstrap are separately composed; they must not be conflated with ordinary seed windowing.

**Required correction:** Preserve the old clamp as clearly labeled historical/replaced behavior, remove the still-pending removal instruction, and add the current forwarding contract: per-open seed_max_chars overrides the gateway budget, both use Meerkat LiveSeedWindow, and bounded projection preserves an affordable existing compaction summary plus a complete-turn suffix with explicit windowed/degraded status. State that this is a serialized-character budget, not a universal provider token-limit guarantee or a newly generated summary. Point separately to console Concurrent summary bootstrap. Do not promise unconditional preservation of every system row.

### Changes and final verification

**Changed:** `docs/design/live-sessions.md`.

Retained the oldest-message clamp only as a historical replaced stopgap. Described current per-open-over-gateway seed_max_chars precedence, forwarding to Meerkat LiveSeedWindow, affordable existing summary plus complete-turn suffix selection, and explicit Windowed projection status. Distinguished serialized-character budgeting from provider token limits and ordinary projection from fresh Concurrent console summarization; removed the obsolete pending removal instruction.

**Validation:** PASS: inspected both ordinary cfg paths in live_wiring.rs, including realtime_projection_messages_with_window and prepare_open_projection. Read exact registry meerkat-0.8.40 live_orchestration.rs selection algorithm and Windowed result. H-007 assertions verify precedence, historical boundary, and absence of the obsolete removal instruction.

**Final review: pass.** The clamp is explicitly retained as replaced history, not current behavior. The new explanation matches both cfg paths and the exact pinned projection algorithm: per-open precedence, serialized-character budgeting, an affordable existing summary, complete-turn suffix selection, and explicit Windowed status. It does not promise fresh summarization, universal token limits, unconditional System retention, or wire exposure of the internal status.

**`docs/design/live-sessions.md:806-819`**

```text
It does not generate a fresh summary or promise to retain every
  System/SystemNotice row.
```

The current forwarding contract is separated from the historical clamp and linked to the separate Concurrent console summary policy.

**`meerkat-mobkit/src/live_wiring.rs:3800-3832,4188-4193,4819-4834`**

```text
let seed_max_chars = parsed.seed_max_chars.or(ctx.seed_max_chars);
```

The ordinary compatibility branch forwards into realtime_projection_messages_with_window; the openai-live branch builds LiveSeedWindow and supplies it to prepare_open_projection.

**`(operator-retained path)/meerkat-0.8.40/src/session_runtime/live_orchestration.rs:584-682`**

```text
status: LiveSeedProjectionStatus::Windowed {
            dropped_messages,
            included_compaction_summary,
        },
```

The implementation first returns Complete if affordable, otherwise optionally retains the latest existing typed summary and a contiguous suffix at conversational-turn boundaries.

**`meerkat-mobkit/src/console_voice/live_host.rs:204-218`**

```text
.with_bootstrap_mode(LiveContextBootstrapMode::Concurrent);
```

Fresh console summary production is a separate composed owner seam, not a side effect of ordinary windowing.

## H-008: The current MDM validation gate instructs a workspace-wide downgrade to Meerkat 0.6.30

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/mdm-mob-target-deployment.md:77-84`**

````text
The deployment helpers now create real target runtimes and real MobKit external
member bindings. The pack is pinned to the Meerkat 0.6.30 family, which includes
the typed bridge reply and production reply-route fixes needed for peer-only
external targets. Validate from `examples/` with:

```bash
npm run mdm:upgrade-meerkat -- 0.6.30
npm run mdm:real-target-smoke
````

Following the current-gate instructions fails the version helper or attempts to replace all direct current Meerkat dependencies with an incompatible older family, rather than validating the documented remote-member path.

**`meerkat-mobkit/Cargo.toml:59-70`**

```text
meerkat-mob = { version = "=0.8.40" }
```

The example builds against the workspace crate and the exact current Meerkat 0.8.40 family, not an independent 0.6.30 pack pin.

**`examples/004-mdm-console-pack/scripts/use-meerkat-version.sh:14-20`**

```text
crate_manifest="${repo_root}/meerkat-mobkit/Cargo.toml"
```

The advertised upgrade command edits the shared core manifest, not just an example fixture or local smoke setting.

**`examples/004-mdm-console-pack/scripts/use-meerkat-version.sh:43-45`**

```text
if ! cargo search meerkat --limit 1 | grep -F "meerkat = \"${version}\"" >/dev/null; then
```

The helper first compares against the registry search's current version and refuses a nonmatching requested release. Even before potential manifest changes, 0.6.30 is not a valid instruction for validating this current checkout.

**`examples/004-mdm-console-pack/scripts/real-target-smoke.sh:49-52`**

```text
./scripts/repo-cargo build -p meerkat-mobkit --example mdm_mob_target
```

The smoke gate already builds the target from this checkout with its checked-in dependencies; no repin step is part of the test.

### Independent adjudication

Confirmed. Unlike the historical design records elsewhere in this scope, this section is explicitly the Current Integration Gate and gives a command to run now. The example's npm alias invokes a script that discovers and edits all direct shared Meerkat manifest dependencies, then updates the lock, whereas the smoke script builds the checked-in example directly. The manifest pins 0.8.40, so 0.6.30 is not the current pack dependency. I did not execute the mutating helper, smoke test, provider calls or deployment commands; the helper's current registry-search result was not measured and need not be asserted to prove the bad validation recipe.

**`docs/design/mdm-mob-target-deployment.md:75-85`**

```text
npm run mdm:upgrade-meerkat -- 0.6.30
```

The historical version is promoted to an active validation prerequisite under Current Integration Gate.

**`meerkat-mobkit/Cargo.toml:59-70`**

```text
meerkat-mob = { version = "=0.8.40" }
```

The example's workspace crate resolves the current family, not a separate old pack pin.

**`examples/package.json:13-16`**

```text
"mdm:upgrade-meerkat": "004-mdm-console-pack/scripts/use-meerkat-version.sh"
```

The documented npm command reaches the actual shared-manifest mutation script.

**`examples/004-mdm-console-pack/scripts/use-meerkat-version.sh:14-20`**

```text
crate_manifest="${repo_root}/meerkat-mobkit/Cargo.toml"
```

Its target is the shared core manifest, not an isolated demo configuration.

**`examples/004-mdm-console-pack/scripts/use-meerkat-version.sh:87-90`**

```text
./scripts/repo-cargo update -p "$crate" --precise "$version"
```

After rewriting pins the helper resolves each dependency at the supplied release; this is a version-changing operation, not test setup.

**`examples/004-mdm-console-pack/scripts/real-target-smoke.sh:49-52`**

```text
./scripts/repo-cargo build -p meerkat-mobkit --example mdm_mob_target
```

The validation command already builds this checkout using its committed dependency selection.

**Required correction:** Describe the pack as using the workspace's checked-in exact Meerkat family (0.8.40 at the audited baseline). Retain 0.6.30 only as a historical statement about the original bridge fixes. Remove mdm:upgrade-meerkat from the current validation recipe; leave npm run mdm:real-target-smoke from examples and/or npm --prefix examples run mdm:real-target-smoke from the root. Do not modify the helper, dependency manifests or lockfile for this documentation correction.

### Changes and final verification

**Changed:** `docs/design/mdm-mob-target-deployment.md`.

Replaced the current 0.6.30 pack pin claim with the workspace's checked-in exact family, 0.8.40 at this baseline. Kept 0.6.30 only as original bridge-fix history. Removed mdm:upgrade-meerkat from the current gate and retained both examples-directory and repository-root forms of the existing smoke command.

**Validation:** PASS: checked examples/package.json's mdm:real-target-smoke alias resolves to an existing script and that the script builds mdm_mob_target through ./scripts/repo-cargo with the checked-in workspace. H-008 assertions verify removal of the mutating prerequisite and retention of the historical note. Neither the version-changing helper nor deployment/smoke commands were executed.

**Final review: pass.** The current MDM gate no longer tells operators to change dependency versions or downgrade. Both documented command forms select the existing smoke alias against the checked-in workspace. The old 0.6.30 bridge-fix account remains explicitly historical. No helper/runtime change or unsupported deployment success claim was introduced.

**`docs/design/mdm-mob-target-deployment.md:75-92`**

```text
checkout does not require changing dependency versions.
```

Only mdm:real-target-smoke remains in the current gate, with examples-directory and root invocation forms.

**`examples/package.json:10-16`**

```text
"mdm:real-target-smoke": "004-mdm-console-pack/scripts/real-target-smoke.sh",
```

The documented alias resolves to the existing smoke script rather than the version-changing helper.

**`examples/004-mdm-console-pack/scripts/real-target-smoke.sh:49-51`**

```text
./scripts/repo-cargo build -p meerkat-mobkit --example mdm_mob_target
```

The smoke builds from the repository's current manifest/lockfile. Both manifests' exact 0.8.40 requirements were checked separately.

## H-009: WorkGraph documentation incorrectly says every mutation carries an expected revision

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/workgraph-integration.md:57-58`**

```text
- CAS: every mutation carries `expected_revision: u64`; conflicts are typed errors.
  Console card must retain per-item `revision`.
```

A client abstraction built on the claimed universal CAS requirement cannot correctly model creation/link/prune and may invent revision parameters that provide no concurrency guarantee.

**`(operator-retained path)/meerkat-workgraph-0.8.40/src/types.rs:1613-1621`**

```text
pub struct LinkWorkItemsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub kind: WorkEdgeKind,
    pub from_id: WorkItemId,
    pub to_id: WorkItemId,
}
```

Link is a mutation but its complete typed request contains no expected_revision.

**`meerkat-mobkit/src/rpc/workgraph_methods.rs:738-746`**

```text
let request: LinkWorkItemsRequest = parse_request(object)?;
```

The gateway forwards this request directly, without adding a CAS field. Create, goal/create, and attention/prune likewise do not require an existing-entity revision.

**`sdk/python/meerkat_mobkit/runtime.py:2440-2453`**

```text
self, kind: str, from_id: str, to_id: str, **kwargs: Any
```

The public SDK mirrors the non-CAS link contract. The wire-contract table already correctly omits expected_revision on link, create, goal/create, and prune, contradicting its own universal sentence at workgraph-wire-contract.md:8.

### Independent adjudication

Confirmed as an already-inaccurate invariant, not a proposal being retrofitted to new APIs. I independently fetched the historical v0.7.23 WorkGraph types via gh api and resolved its annotated tag to commit 0d8f6ff3d159955d596514ffcac4bdd87b246d91. CreateWorkItemRequest, LinkWorkItemsRequest and GoalCreateRequest already lacked expected_revision there. The current pinned link request and direct forwarding preserve that distinction; current attention/prune is another explicitly non-CAS request. Therefore both universal sentences are wrong even within the historical contract's own source version. Preserve the old stage plan and clarify which revision-bearing operations use work-item versus binding-machine revisions.

**`docs/design/workgraph-integration.md:57-58`**

```text
- CAS: every mutation carries `expected_revision: u64`; conflicts are typed errors.
```

The claim appears in the explicitly verified historical upstream survey.

**`docs/design/workgraph-wire-contract.md:5-8`**

```text
upstream shapes). `expected_revision` is the CAS token on every mutation.
```

The binding-contract introduction repeats the same overstatement; its method table itself omits the token for link/create.

**`https://github.com/lukacf/meerkat/blob/0d8f6ff3d159955d596514ffcac4bdd87b246d91/meerkat-workgraph/src/types.rs:1199-1207`**

```text
pub struct LinkWorkItemsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub kind: WorkEdgeKind,
    pub from_id: WorkItemId,
    pub to_id: WorkItemId,
}
```

Independent historical proof: the complete v0.7.23 link request has no revision token. The same fetch showed no token on create at 1083-1109 and goal/create at 1223-1240.

**`(operator-retained path)/meerkat-workgraph-0.8.40/src/types.rs:1613-1621`**

```text
pub struct LinkWorkItemsRequest {
```

The current complete struct still contains scope/kind/from_id/to_id and no expected_revision.

**`(operator-retained path)/meerkat-workgraph-0.8.40/src/types.rs:1991-2002`**

```text
pub struct AttentionPruneRequest {
```

Prune contains scope and optional updated_before only, so it must not be described as per-entity CAS.

**`meerkat-mobkit/src/rpc/workgraph_methods.rs:738-745`**

```text
let request: LinkWorkItemsRequest = parse_request(object)?;
```

MobKit forwards the upstream link shape instead of introducing a universal gateway-level CAS token.

**`console/src/lib/workgraph-actions.ts:80-108`**

```text
const machineState = asRecord(asRecord(asRecord(result)?.attention)?.machine_state);
```

The actual console revision resolvers distinguish goal item.revision from attention.machine_state.revision.

**Required correction:** Narrow both universal CAS sentences: expected_revision is required by the revision-checked mutations of existing work items or attention bindings, not every mutation. Explicitly exclude create, goal/create, link and attention/prune from a universal caller-CAS requirement; do not imply those operations lack all transactional invariants. Item mutations and goal confirm/request_close use the work item's revision; attention pause/resume/reassign use attention.machine_state.revision. Preserve the historical implementation plan and per-method shapes.

### Changes and final verification

**Changed:** `docs/design/workgraph-integration.md`, `docs/design/workgraph-wire-contract.md`.

Corrected both universal CAS statements. Explicitly excluded create, goal/create, link, and attention/prune from caller-supplied CAS while retaining transactional-invariant caveats. Distinguished work-item revisions for item/goal mutations from attention.machine_state.revision for attention pause/resume/reassign. Preserved the implementation plan, historical method table, and as-built notes.

**Validation:** PASS: read-only assertions inspect exact pinned CreateWorkItemRequest, GoalCreateRequest, LinkWorkItemsRequest, and AttentionPruneRequest structs and confirm none contains expected_revision. Checked the adjudicated console revision-resolver evidence. H-009 assertions verify both documents list all exclusions and binding revision source. Historical table/as-built byte-preservation checks pass.

**Final review: pass.** Both universal CAS claims were corrected with the four explicit non-CAS request families and the distinction between item and attention-machine revisions. The transactional-invariant caveat avoids implying that non-CAS requests are unguarded. Historical method tables and the implementation plan were preserved.

**`docs/design/workgraph-integration.md:61-68`**

```text
`goal/confirm`/`goal/request_close` use the work item's `revision`;
```

The same correction appears in workgraph-wire-contract.md:12-18, including create/goal-create/link/prune exclusions.

**`(operator-retained path)/meerkat-workgraph-0.8.40/src/types.rs:1462-1492,1613-1621,1637-1674,1993-2002`**

```text
pub struct LinkWorkItemsRequest {
```

Read the complete CreateWorkItemRequest, LinkWorkItemsRequest, GoalCreateRequest, and AttentionPruneRequest structs: none has expected_revision. By contrast UpdateWorkItemRequest and the cited goal/attention requests require it.

**`console/src/lib/workgraph-actions.ts:82-108`**

```text
const machineState = asRecord(asRecord(asRecord(result)?.attention)?.machine_state);
```

Goal-confirm/request-close resolution reads item.revision, whereas attention pause/resume/reassign resolution reads attention.machine_state.revision.

## H-010: The still-referenced WorkGraph binding contract lacks the current immutable namespace-grant restriction

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/workgraph-wire-contract.md:16-20`**

```text
| `mobkit/workgraph/snapshot` | `{namespace?, all_namespaces?, statuses?: string[], labels?: string[], include_terminal?, limit?}` | `WorkGraphSnapshot` |
| `mobkit/workgraph/list` | same filter | `{items: WorkItem[]}` |
| `mobkit/workgraph/get` | `{id, namespace?}` | `{item: WorkItem}` |
| `mobkit/workgraph/ready` | `{namespace?, labels?, limit?}` | `{items: WorkItem[]}` |
| `mobkit/workgraph/events` | `{namespace?, all_namespaces?, after_seq?, limit?}` | `{events: WorkGraphEvent[]}` |
```

A reader using this still-referenced contract for the currently pinned SDK sends all_namespaces=true or a sidecar namespace and gets a hard refusal, or searches durable rows under the wrong bare realm ID. This finding does not claim that the explicitly versioned 0.7.30 behavior was historically false.

**`sdk/typescript/src/runtime.ts:2316-2321`**

```text
// converted at the call site. See docs/design/workgraph-wire-contract.md.
```

Current SDK source still directs implementers to this document as its contract; the opening says 'Binding contract', not archived/superseded. The correction should retain its versioned history but delimit present applicability.

**`(operator-retained path)/meerkat-workgraph-0.8.40/src/service.rs:1618-1623`**

```text
"all_namespaces requires a separate host capability; a namespace grant authorizes exactly one immutable namespace"
```

events rejects all_namespaces=true; normalize_item_filter and normalize_snapshot_filter at lines 1653-1692 make the same rejection for list and snapshot.

**`(operator-retained path)/meerkat-workgraph-0.8.40/src/service.rs:1635-1650`**

```text
if realm_id != self.namespace_grant.realm_id
            || namespace.as_str() != self.namespace_grant.namespace
```

Every service operation using scope is restricted to the immutable grant, not just the goal/attention subset singled out at document lines 168-170. Nondefault item namespaces do not pass through.

**`meerkat-mobkit/src/workgraph_wiring.rs:107-123`**

```text
let realm = match meerkat_core::mob_realm_id(mob_id) {
```

Current scope uses the canonical mob.<mob_id> realm rather than the bare mob definition ID described at document lines 140-142. The service receives WorkNamespace::default().

**`meerkat-mobkit/tests/workgraph_rpc.rs:251-254`**

```text
item["realm_id"],
        json!(format!("mob.{}", runtime_mob_id(&runtime)))
```

The current RPC test asserts the exact realm visible in actual returned rows.

**`meerkat-mobkit/src/rpc/workgraph_methods.rs:177-177`**

```text
WorkGraphError::InvalidInput(_) => invalid_params(detail),
```

Grant and all_namespaces refusals surface as JSON-RPC -32602, not a successful broad query or a generic backend error.

### Independent adjudication

Confirmed only as a missing current-applicability qualification on a still-referenced binding contract. The title and 'as-built, 0.7.30' section are real historical boundaries: this review does not claim that old all_namespaces or bare-realm behavior was historically false. However, current SDK and dispatcher source direct readers here and there is no supersession/current-scope warning. The pinned service rejects all_namespaces=true on list/snapshot/events and refuses any scope different from its immutable grant; default MobKit construction now derives mob.<mob_id> and WorkNamespace::default(). Narrow the initial proposed correction for library injection: with_workgraph_service accepts a supplied service, so 'every possible service must use literal default namespace' would itself be too broad.

**`docs/design/workgraph-wire-contract.md:3-8`**

```text
Binding contract for the `mobkit/workgraph/*` JSON-RPC group, the experience
```

The page is still presented as a binding contract, although version-titled.

**`sdk/typescript/src/runtime.ts:2318-2321`**

```text
// converted at the call site. See docs/design/workgraph-wire-contract.md.
```

This current SDK reference gives a concrete reason current callers will use the old method table, not merely archival curiosity.

**`(operator-retained path)/meerkat-workgraph-0.8.40/src/service.rs:1618-1623`**

```text
all_namespaces requires a separate host capability; a namespace grant authorizes exactly one immutable namespace
```

The events filter rejects true. normalize_item_filter and normalize_snapshot_filter at 1653-1692 independently impose the same restriction.

**`(operator-retained path)/meerkat-workgraph-0.8.40/src/service.rs:1635-1650`**

```text
if realm_id != self.namespace_grant.realm_id
```

scope also compares the namespace to the grant and returns InvalidInput for a different realm/namespace; the restriction is not confined to attention methods.

**`meerkat-mobkit/src/workgraph_wiring.rs:103-123`**

```text
let realm = match meerkat_core::mob_realm_id(mob_id) {
```

Stock composition derives canonical mob.<mob_id> and then calls WorkGraphService::with_scope(store, realm, WorkNamespace::default()). Invalid mob IDs have an explicit error/fallback path, not a supported alternate public realm spelling.

**`meerkat-mobkit/src/rpc/workgraph_methods.rs:177-177`**

```text
WorkGraphError::InvalidInput(_) => invalid_params(detail),
```

The service's grant refusal becomes the standard -32602 invalid-params error.

**`meerkat-mobkit/src/mob_handle_runtime.rs:6740-6743`**

```text
self.workgraph_service = service;
```

Library embedders can inject a composed service; its granted default scope, rather than a universal literal namespace requirement, is the authoritative qualification.

**Required correction:** Preserve the 0.7.30 as-built account and add a clearly visible current-contract qualification/appendix for the audited Meerkat 0.8.40 dependency. Stock gateway/default MobKit construction uses canonical mob.<mob_id> with the default namespace. All WorkGraph reads/mutations remain inside the service's immutable namespace grant; optional namespace may name only that granted scope. all_namespaces=true on list/snapshot/events and out-of-grant namespaces are refused with -32602; realm_id remains caller-forbidden. For a library-injected service, describe its own fixed grant rather than claiming every service must use the stock namespace. Do not silently rewrite the historical table as if these restrictions existed in 0.7.30 or suggest widening the grant.

### Changes and final verification

**Changed:** `docs/design/workgraph-wire-contract.md`, `docs/design/workgraph-integration.md`.

Added a prominent current Meerkat 0.8.40 scope qualification while retaining the 0.7.30 inventory and as-built account. Documented stock mob.<mob_id>/default namespace construction, all-operation immutable grants, -32602 for all_namespaces=true and out-of-grant namespaces, and caller-forbidden realm_id. Qualified library-injected services by their own fixed grant. Added a link from the historical integration plan and a warning adjacent to the old as-built scope notes.

**Validation:** PASS: inspected workgraph_wiring.rs::scoped_workgraph_service and grant forwarding, rpc/workgraph_methods.rs namespace checks/error mapping, and pinned meerkat-workgraph-0.8.40 service.rs scope/events/list/snapshot normalization. H-010 assertions cover stock and injected-service boundaries. Both new relative qualification links resolve, and historical scope bullets remain byte-identical.

**Final review: pass.** The new current qualification is prominent, linked from the integration plan, and repeated beside the historical notes. It correctly describes canonical stock construction, service-wide immutable grants, -32602 scope refusals, and caller-forbidden realm_id without forcing a library-injected service to use the stock namespace. Historical bare-realm and narrower-scope notes remain explicitly superseded history rather than silently rewritten facts.

**`docs/design/workgraph-wire-contract.md:20-38,165-169`**

```text
Library embedders can inject a composed `WorkGraphService`; its own fixed
  grant is authoritative rather than a universal requirement to use the stock
  namespace.
```

This preserves the adjudicated embedder qualification while restricting public callers to the composed grant.

**`meerkat-mobkit/src/workgraph_wiring.rs:107-123,153-163`**

```text
WorkGraphService::with_scope(store, realm, WorkNamespace::default())
```

Stock composition derives realm through mob_realm_id and forwards the service's own namespace grant to tools.

**`(operator-retained path)/meerkat-workgraph-0.8.40/src/service.rs:1614-1692`**

```text
all_namespaces requires a separate host capability; a namespace grant authorizes exactly one immutable namespace
```

Events/list/snapshot refuse all_namespaces=true; scope rejects any realm/namespace unequal to the immutable grant.

**`meerkat-mobkit/src/rpc/workgraph_methods.rs:119-124,166-177,276-292,570-577`**

```text
WorkGraphError::InvalidInput(_) => invalid_params(detail),
```

The mapped invalid-params code is -32602; goal/attention checks compare to the service's default namespace and realm_id is explicitly rejected.

**`meerkat-mobkit/src/mob_handle_runtime.rs:6740-6743`**

```text
self.workgraph_service = service;
```

Library injection accepts an existing composed service rather than reconstructing its scope as the stock default.

## H-011: WorkGraph conflict handling incorrectly attributes refetch-and-retry to the SDK

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/workgraph-wire-contract.md:51-53`**

```text
- CAS/revision conflict (upstream Conflict): code `-32042`,
  `data.kind = "workgraph_conflict"`, `data.detail` carries upstream message —
  SDKs and console retry by refetching revision.
```

Hosts may omit required conflict handling, assuming a mutation already retried and succeeded; users may expect a console action to complete automatically when it actually failed and only refreshed its displayed state.

**`sdk/python/meerkat_mobkit/runtime.py:2370-2381`**

```text
raw = await self._runtime._rpc("mobkit/workgraph/update", params)
```

The Python mutation does one RPC using the caller's expected_revision and returns/parses it; no refetch or retry loop exists here.

**`sdk/python/meerkat_mobkit/errors.py:237-243`**

```text
upstream WorkGraph error message). Callers should refetch the item's or
    binding's current ``revision`` and retry.
```

The typed error's own public contract assigns recovery to callers, not the SDK.

**`sdk/typescript/src/runtime.ts:913-914`**

```text
throw new WorkGraphConflictError(message, rid, method, err.data);
```

TypeScript also throws the typed conflict; workgraphUpdate at lines 2413-2423 makes a single call with expectedRevision.

**`console/src/ConsoleApp.tsx:3589-3597`**

```text
// path (marked `refresh` so the failure flag survives): the NEXT
        // action CASes against the live revision. Best-effort — the banner
```

The console catches the failure and performs a best-effort read for an inline card, but it does not automatically retry the failed mutation. The next explicit action uses refreshed state.

### Independent adjudication

Confirmed after tracing past the convenience methods into both SDK RPC transports and the console catch path. Both SDKs issue a request and raise the typed conflict; neither performs entity refetch or write retry. The console echoes the failure, best-effort fetches current state for an inline card and stops; the next explicit operator action uses refreshed state. Refetch-before-write when a revision was never observed is separate behavior and does not prove failed writes automatically retry. The correction should assign recovery to callers without implying every -32042 conflict can be cured by changing a revision (the server also uses that code for occupancy/stale-witness conflicts).

**`docs/design/workgraph-wire-contract.md:51-53`**

```text
SDKs and console retry by refetching revision.
```

The claim assigns successful recovery behavior to the libraries/UI rather than to their callers.

**`sdk/python/meerkat_mobkit/runtime.py:708-728`**

```text
raise _rpc_error_from_payload(response["error"], request_id=rid, method=method)
```

Python's asynchronous RPC core propagates the response error immediately; the mapping at 184-191 constructs WorkGraphConflictError.

**`sdk/python/meerkat_mobkit/errors.py:237-243`**

```text
upstream WorkGraph error message). Callers should refetch the item's or
```

The typed error explicitly gives refetch/retry responsibility to callers.

**`sdk/typescript/src/runtime.ts:876-915`**

```text
throw new WorkGraphConflictError(message, rid, method, err.data);
```

TypeScript sends once and raises the conflict rather than entering a refetch/retry loop.

**`sdk/typescript/src/runtime.ts:2413-2423`**

```text
const raw = await this._runtime._rpc("mobkit/workgraph/update", params);
```

The public mutation supplies the caller's expectedRevision to that single RPC, with no outer retry loop.

**`console/src/ConsoleApp.tsx:3581-3620`**

```text
if (cardIdentity && jsonRpcErrorCode(err) === WORKGRAPH_CONFLICT_CODE) {
```

The catch block surfaces the action failure, executes only the derived read refresh, folds refreshed state, and exits without reissuing the mutation.

**Required correction:** Keep -32042/workgraph_conflict and its structured detail. State that both SDKs raise WorkGraphConflictError and do not automatically retry; callers inspect the conflict, refetch the relevant item/binding state, reconsider the intended operation, and explicitly retry with the appropriate current revision when still valid. The console displays the failure and best-effort refreshes the inline card for a subsequent operator action; it does not automatically replay the failed write. Do not promise revision refresh resolves every conflict.

### Changes and final verification

**Changed:** `docs/design/workgraph-wire-contract.md`.

Assigned conflict recovery to callers: both SDKs raise WorkGraphConflictError without automatic retry; callers inspect/refetch/reconsider and explicitly retry valid operations with the appropriate revision. Clarified console failure display and best-effort inline refresh for a subsequent action, not write replay, and warned revision refresh does not solve every conflict.

**Validation:** PASS: inspected ConsoleApp.tsx catch/refresh path and checked SDK error propagation plus the adjudicated single-request mutation/transport paths. H-011 assertions confirm typed errors, no automatic retries/write replay, and the non-revision conflict caveat.

**Final review: pass.** The final error text correctly assigns recovery to callers, preserves -32042/workgraph_conflict/detail, distinguishes console best-effort refresh from failed-write replay, and warns that not every conflict is solved by a new revision. Both SDK request paths and the full console catch path support these statements.

**`docs/design/workgraph-wire-contract.md:83-91`**

```text
Both SDKs raise `WorkGraphConflictError`; neither automatically retries.
```

Callers must inspect/refetch/reconsider and explicitly retry only when appropriate; console refresh is for the next action.

**`sdk/python/meerkat_mobkit/runtime.py:184-190,708-728`**

```text
raise _rpc_error_from_payload(response["error"], request_id=rid, method=method)
```

A single async request raises the mapper's typed WorkGraphConflictError rather than refetching or resubmitting.

**`sdk/typescript/src/runtime.ts:876-922,2413-2422`**

```text
throw new WorkGraphConflictError(message, rid, method, err.data);
```

The mutation wrapper and shared transport make one request and throw the conflict; neither contains a retry loop.

**`console/src/ConsoleApp.tsx:3579-3623`**

```text
if (cardIdentity && jsonRpcErrorCode(err) === WORKGRAPH_CONFLICT_CODE) {
```

The catch records the failure, optionally executes a separate refresh query, folds fresh card state, and exits without replaying the mutation.

**`meerkat-mobkit/src/rpc/workgraph_methods.rs:150-177`**

```text
WorkGraphError::StaleRevision { .. } | WorkGraphError::Conflict(_) => {
```

The conflict code includes more than revision mismatch and also covers the explicitly classified stale-authority witness, supporting the recovery caveat.

## Final scope checks

> [
>   "Read audit-brief.md, the H manifest entry, audit-H.json, adjudication-H.json, fixes-H.json, all eight H documents, and the complete final H diff. Independently chased all eleven decisions into local source and the exact cached registry dependencies.",
>   "Ledger check: audit/adjudication/fixes contain the same eleven H IDs; all are confirmed, none rejected, and no supplemental findings are pending.",
>   "PASS: git diff --check restricted to all eight H-owned documents.",
>   "PASS: all eight documents are regular files, Markdown fences are balanced, and all seven relative file/anchor links resolve.",
>   "PASS: exactly five owned documents differ from af82b6b3ab34faed9bf3e962d148d55f10dcd1dc; hub-wire-contract.md, schedule-domain-adoption.md, and voice-handover.md are unchanged.",
>   "PASS: byte comparisons preserve identity D1/D2, the dated deployment census, phase plan, WorkGraph historical method inventory, and all historical as-built bullets. Proposals and the emergency voice handover were not treated as current implementation or fresh acceptance evidence.",
>   "PASS: independent TOML check found MobKit 0.8.39, 25 exact =0.8.40 dependency declarations across both manifests (19 unique directly named upstream crates), and 35 resolved registry Meerkat-family packages at 0.8.40. These are the independently measured counts; fixes-H.json's abbreviated 18/19 validation counts should not be repeated as exhaustive totals.",
>   "PASS: exact seven HTTP channel methods, principal/identity/channel ownership, empty HTTP feature_capabilities, stdin generic routing, voice -32050/voice_unavailable, close-timeout voice_busy, and WorkGraph -32602/-32042 mappings checked against source.",
>   "PASS after residual-fix independent re-review: H-006 now distinguishes rejected nested execution_identity unknown fields from ignored unknown top-level capability fields; H-R001 is resolved. Initial failure and complete regression evidence are retained in review_history.",
>   "PASS: all eleven current item evidence quotes revalidated against their cited line ranges; H-006 quotation and H-007 shifted range refreshed. The corrected Live page passes git diff --check, balanced-fence checks, and relative file/anchor checks.",
>   "No repository edits, commits, dependency changes, delegation, builds, runtime/SDK suites, MDM smoke/deployment, provider calls, or paid/browser audio acceptance were performed. Read-only structural/source verification does not establish runtime acceptance."
> ]

## Review feedback and resolution history

Earlier review failures are retained here; the per-item dispositions above reflect the final re-review rather than erasing the feedback.

```json
[
  {
    "phase": "initial_wave4_review",
    "summary": {
      "pass": 10,
      "fail": 1,
      "unresolved_regressions": 1
    },
    "failed_items": [
      {
        "id": "H-006",
        "verdict": "fail",
        "reason": "The main strict-versus-ordinary distinction is fixed, including model/provider pairing, instructions, turning_mode, and the console exclusion. One residual rejection overclaim remains in the rewritten bullet: it says strict open rejects caller-supplied 'capability claims' without confining this to the strict nested execution_identity envelope. Extra top-level properties such as capabilities or feature_capabilities are not in the strict surface denylist and are ignored by GatewayLiveOpenParams. They cannot grant capability authority, but ignoring them is not rejecting the request. Narrow this clause to the nested envelope or say capability authority remains host-owned; do not alter runtime behavior.",
        "evidence": [
          {
            "path": "docs/design/live-sessions.md",
            "lines": "791-801",
            "quote": "caller-supplied instructions, execution-mode overrides, model, provider,\n  tools, Responses configuration, and capability claims.",
            "explanation": "The rewritten method-level rejection claim still encompasses capability properties without distinguishing nested validation from tolerant top-level parsing."
          },
          {
            "path": "meerkat-mobkit/src/live_contracts.rs",
            "lines": "124-138,345-367,374-410",
            "quote": "#[serde(deny_unknown_fields)]\npub struct LiveExecutionIdentityV1 {",
            "explanation": "Unknown fields are rejected inside execution_identity, whose fields are version/profile_id. The separate top-level strict denylist enumerates instructions/tools/Responses/mode fields, but neither capabilities nor feature_capabilities."
          },
          {
            "path": "meerkat-mobkit/src/live_wiring.rs",
            "lines": "1544-1582",
            "quote": "/// before `handle_live_method`; unknown fields are ignored here so the\n/// target spellings pass through untouched.",
            "explanation": "GatewayLiveOpenParams uses ordinary Deserialize without deny_unknown_fields and has no capability-claim field; extra top-level capability properties are discarded."
          },
          {
            "path": "meerkat-mobkit/src/rpc.rs",
            "lines": "5703-5720",
            "quote": "Ok(Some(_)) if capability_available => None,",
            "explanation": "After nested parsing and the explicit surface denylist, an otherwise valid configured strict request proceeds. There is no additional top-level unknown-field rejection here."
          },
          {
            "path": "meerkat-mobkit/src/live_wiring.rs",
            "lines": "4698-4706,4800-4857",
            "quote": "projection.append_ephemeral_system_overlay(overlay);",
            "explanation": "The strict host receives only typed selected fields, while the ordinary path genuinely supports the newly documented instructions/model/paired-provider/turning_mode controls. The failure is narrowly about rejection wording, not authority escape or the main scope correction."
          }
        ]
      }
    ],
    "regressions": [
      {
        "id": "H-R001",
        "related_item": "H-006",
        "severity": "low",
        "classification": "retained overclaim in rewritten prose, not a runtime regression",
        "title": "Strict-open rejection wording still overstates unknown top-level capability-field validation",
        "doc": {
          "path": "docs/design/live-sessions.md",
          "lines": "791-794",
          "quote": "rejects\n  caller-supplied instructions, execution-mode overrides, model, provider,\n  tools, Responses configuration, and capability claims."
        },
        "evidence": [
          {
            "path": "meerkat-mobkit/src/live_contracts.rs",
            "lines": "345-410",
            "quote": "serde_json::from_value(raw.clone())",
            "explanation": "Strict unknown-field decoding applies to raw execution_identity only; the separate top-level denylist does not include capabilities or feature_capabilities."
          },
          {
            "path": "meerkat-mobkit/src/live_wiring.rs",
            "lines": "1544-1582,4698-4706",
            "quote": "/// before `handle_live_method`; unknown fields are ignored here so the\n/// target spellings pass through untouched.",
            "explanation": "Top-level capability properties are dropped by the tolerant open-params deserializer; only selected typed fields reach the shared strict owner."
          }
        ],
        "impact": "A caller or conformance test can incorrectly expect an invalid-params refusal for an otherwise valid strict open containing extra top-level capability properties. The fields confer no authority; this is an error-contract documentation mismatch, not a demonstrated capability bypass.",
        "correction": "Retain the strict-versus-ordinary distinction. Confine unknown/capability-field rejection to the nested execution_identity envelope, and state separately that capabilities remain host-owned and cannot be granted by callers. Do not promise rejection of every unknown top-level property and do not change the runtime parser for this documentation task.",
        "validation": "Source trace only: nested preflight, full top-level denylist, tolerant GatewayLiveOpenParams, and typed shared-host call were inspected. No provider open or new runtime test was performed."
      }
    ],
    "citation_context": "The failed-item document quotes and line ranges in this history describe the pre-residual-fix text, intentionally preserved rather than updated to erase the finding."
  },
  {
    "phase": "residual_fix_independent_rereview",
    "date": "2026-09-22",
    "fix_report": "fixes-review-residuals.json",
    "resolutions": [
      {
        "id": "H-006",
        "related_regression": "H-R001",
        "verdict": "pass",
        "status": "resolved",
        "reason": "The new prose explicitly limits unknown-field rejection to the nested execution_identity envelope and says extra top-level capabilities/feature_capabilities are ignored, not rejected, and cannot grant authority. Independently reread the nested deserializer, full strict top-level denylist, tolerant open-params shape, preflight, typed host invocation, and ordinary override branch; all support the correction.",
        "evidence": [
          {
            "path": "docs/design/live-sessions.md",
            "lines": "794-798",
            "quote": "Unknown fields inside the nested\n  `execution_identity` envelope, including capability claims, are rejected.",
            "explanation": "The following sentence expressly contrasts ignored top-level capability fields; the overbroad method-level rejection statement is gone."
          },
          {
            "path": "meerkat-mobkit/src/live_contracts.rs",
            "lines": "124-138,345-410",
            "quote": "#[serde(deny_unknown_fields)]\npub struct LiveExecutionIdentityV1 {",
            "explanation": "The actual wire selector is execution_identity, containing version/profile_id. The top-level execution_profile spelling is explicitly forbidden, not the name of this nested envelope."
          },
          {
            "path": "meerkat-mobkit/src/live_wiring.rs",
            "lines": "1544-1582,4698-4706",
            "quote": "/// before `handle_live_method`; unknown fields are ignored here so the\n/// target spellings pass through untouched.",
            "explanation": "Unknown top-level capability properties remain inert rather than request-rejecting; only selected typed fields reach the shared strict owner."
          }
        ]
      }
    ],
    "citation_refresh": [
      "H-006 current evidence now cites the corrected text at docs/design/live-sessions.md:791-805.",
      "H-007's unchanged seed-window paragraph moved by four lines; its current evidence range is refreshed to docs/design/live-sessions.md:806-819.",
      "All other current item evidence quotes were rechecked against their cited source/document line ranges."
    ],
    "summary": {
      "pass": 11,
      "fail": 0,
      "unresolved_regressions": 0
    }
  }
]
```
