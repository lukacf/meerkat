# Wave (a) Straggler Report

> **Current status (0.6.5):** Historical audit record. It preserves the
> wave-a/wave-b vocabulary and line references from a broken intermediate tree;
> later releases removed or replaced many cited realtime and runtime surfaces.
> Use `docs/reference/architecture`, `docs/guides/realtime`, and generated
> schemas for current contracts.

**Scope:** read-only audit of `dogma/wave-a-demolition` branch against the 70-violation dogma catalog at `/Users/luka/.codex/dogma-violations.md` and the wave (a) agent completion set (tasks #1–#8).

**Agents:** `core-loop`, `runtime`, `mob`, `foundations`, `session-surface`, `factory-creds`, `rpc-servers`, `rest-mcp-cli-docs`.

**Tree state:** intentionally broken after demolition pass (no-compile). This report enumerates what wave (a) deleted vs. what still survives; it is **not** a re-delete plan and it is not normalized against tombstones that wave (b)/(c) will rebuild.

Unless noted, line numbers below are **current HEAD**, not the 70-row scan's drifted lines.

---

## Section 1 — Surviving violations from the 70-row scan

Legend for "State":
- **Gone** — violation-described symbol/behavior fully removed.
- **Partial** — described behavior mostly gone but one or more fragments survive.
- **Survives** — violation is still intact or largely untouched.
- **Tombstone** — referenced symbol is deleted but callers/comments still reference it (breaks compile).

Rows below list only violations with state ≠ Gone.

| # | Title | State | File:Line | Owner | Notes |
|---|---|---|---|---|---|
| 3 | Runtime turn-override seam still has no single canonical carrier | Partial | `meerkat-core/src/lifecycle/run_primitive.rs:95,143,155` (RuntimeTurnMetadata still fields `model`/`provider`/`connection_ref`/etc); `meerkat-runtime/src/runtime_loop.rs:85,92` (two separate RuntimeTurnMetadata construction sites) | core-loop | `RuntimeTurnMetadata` remains the per-turn override carrier. `runtime_backed.rs:199` added a hard-fail on ad-hoc construction but the wire type itself still carries the six overrides. |
| 4 | Turn-limit terminalization diverges between standalone and runtime-backed paths | Partial | `meerkat-core/src/agent/state.rs:424,612` still calls `handle.turn_limit_reached()` via `TurnExecutionInput::TurnLimitReached` | core-loop | Field-level scan only finds the input plumbing. Agents claim the divergence is gone (see commit `fdb569aaf`), but no assertion test proves parity. Suggest sanity check. |
| 6 | Raw session-store rows still escape as authoritative durable state | Likely Gone | `meerkat-session/src/persistent.rs:1298,1302,1343,1983` — the violation's original lines no longer contain raw-row escapes; the callsites are in streaming-append (line 1290–1310) which reconciles through runtime-aware helpers | session-surface | Re-audit after rebuild. Mob session_service references are still present (`meerkat-mob/src/runtime/session_service.rs:433,441`), appear to load rows — verify. |
| 8 | Ambient provider credential selection without `connection_ref` | Partial | `meerkat/src/factory.rs:1684,2306` — `match … { None => { … } }` branches still exist when callers omit `connection_ref`; did the fallback change from "pick first" or did the whole path collapse? `1aa0c0f15` commit says deleted "pre-resolution provider tool defaults", `28e7a51c1` says deleted "ambient provider credential selection" | factory-creds | Factory still has `None =>` arms at 1241, 1684, 1819, 2306, 2884, 2900. Re-read required; might have been replaced by error, not by canonical path. |
| 14 | Builtin skill tools fabricate `canonical_key` by parsing `SkillId` strings | **Tombstone** | `meerkat-tools/src/builtin/skills/{browse.rs:57,135, load.rs:71, resources.rs:84,130, functions.rs:80}` — all six sites call `canonical_key(&…)` but the function body is deleted (no `fn canonical_key` anywhere in `meerkat-tools/` or `meerkat-core/skills/`) | foundations | 6 unresolved calls. Either callers must be collapsed into `SkillId.0` JSON fields directly, or a typed `SourceIdentityRegistry::canonical_skill_key` needs wiring. |
| 19 | Realtime channel status semantics drift between RPC/MCP and WS | Partial | `meerkat-rpc/src/realtime_ws.rs:2644,2652,2659,2666,2673` (WS sets `attempt_count: 0/1` inline); `meerkat-mob-mcp/src/public_mcp.rs:216,220,229` (MCP still hand-builds `RealtimeChannelStatus` with its own `attempt_count`) | rpc-servers | Two code paths still independently mint `RealtimeChannelStatus`. Vocabulary hasn't been collapsed. |
| 22 | Public mob MCP results leak raw runtime incarnation handles | Survives | `meerkat-mob/src/runtime/handle.rs:226,228,230,322,324,363,365` — `agent_runtime_id: AgentRuntimeId` and `fence_token: FenceToken` still public on multiple result types | mob | The MCP-side (`public_mcp.rs`) serialization of `agent_runtime_id`/`fence_token` is gone from greps, but the domain types they serialize from still expose these fields publicly. Re-audit whether any public surface still pokes them. |
| 23 | Member lifecycle and terminal truth materialized in helper code | Gone (file deleted) | `meerkat-mob/src/runtime/mob_member_lifecycle_authority.rs` — file deleted (commit `949962121`) | mob | Verify downstream callers converted. `handle.rs:3021,3040,3300` originals — re-read `handle.rs` at current lines to confirm. |
| 24 | Kickoff state back-filled via `dsl_authority.state` mutation | Partial | `meerkat-mob/src/runtime/actor.rs:701,719` still mutates `self.dsl_authority.state.lifecycle_phase` directly (project_dsl_phase path); `742,794,805,815` still runs `persist_kickoff_state` and writes kickoff into roster | mob | Pattern `self.dsl_authority.state.lifecycle_phase = …` is still present; confirm these are "project from DSL", not back-fill. The roster kickoff persist path looks legitimate. |
| 26 | Public runtime/session control nouns remain surface-level API | Partial | `docs/api/rpc.mdx:79,82,168,249` (still describes `session/status`, `session/submit`, `session/retire`, `session/reset`, `session/submission`, `session/submissions`); `sdks/python/meerkat/client.py:2071,2149` (`async def status`, `async def submit`); `artifacts/schemas/rpc-methods.json:241` | rest-mcp-cli-docs | TS SDK `status()`/`submit()` methods are gone (grep clean). Python SDK + docs still expose them. Contract + catalog cleanup left dangling. |
| 29 | Canonical `connection_ref` fragmentation across CLI/MCP/web/REST | Partial | `meerkat-cli/src/main.rs:1381` ("either `realm:binding` or bare `binding`") still doc-advertises the string form; CLI Logout/Refresh still parses string `profile_id` | rest-mcp-cli-docs | Web + TS SDK paths likely cleaned (grep quiet). CLI string-parse holdout. |
| 30 | REST read endpoints mutate runtime attachment state | Partial | `meerkat-rest/src/lib.rs:1280,1915,1986,2663,3495` still call `ensure_runtime_session_registered` on GET-adjacent paths | rest-mcp-cli-docs | Mutation helper still invoked from 5 call sites, at least one (3495) is in a non-write handler. Flag for wave (b) verification. |
| 34 | Public docs drift from shipped surface | Partial | `docs/api/rpc.mdx:79-289,888-912`; `artifacts/schemas/rpc-methods.json:241` still advertise `session/submit`, `session/status`, `session/retire`, `session/reset` | rest-mcp-cli-docs | Docs not regenerated after SDK/surface delete. |
| 40 | Kernel/schema truth stringly typed | Survives | `meerkat-machine-schema/src/composition.rs:53,262,286,305` — `producer_instance: String`, `route_name: String`, etc. all intact | (none assigned) | `meerkat-machine-schema` crate has 0 LOC delta this wave. |
| 41 | RMAT routed-seam coverage is non-semantic | Survives | `xtask/src/rmat_audit.rs:443` still treats routed effect realization as rule-only; `xtask/src/rmat_policy.rs:65,79,161` | (none assigned) | `xtask/` crate has 0 LOC delta. |
| 42 | Seam inventory omits routed machine seams & no CI gate | Survives | `xtask/src/seam_inventory.rs:189,193,342`; `Makefile:206,211`; `.github/workflows/ci.yml` still no seam-inventory lane | (none assigned) | Untouched. |
| 43 | AuthMachine hard-authority coverage excluded from RMAT | Survives | `xtask/src/rmat_policy.rs` (no AuthMachine in module list); `xtask/src/rmat_audit.rs` untouched | (none assigned) | Untouched. |
| 44 | Ownership-ledger omits AuthMachine/auth-lease family | Survives | `xtask/src/ownership_ledger.rs`, `docs/architecture/finite-ownership-ledger.md` untouched | (none assigned) | Untouched. |
| 45 | Seam-inventory explicit classification debt | Survives | `xtask/src/seam_inventory.rs:161,176` — "Heuristic fallback" comments still present | (none assigned) | Untouched. |
| 47 | Turn-scoped `additional_instructions` flattened into prompt folklore | Partial | `meerkat-session/src/ephemeral.rs` diff −22 lines (commit `3c463e13d`); need to re-verify `meerkat-core/src/service/mod.rs:720,724` | session-surface | ephemeral path mostly cleaned. SessionService seam may still describe `additional_instructions` without backing. |
| 49 | Live hot-swap leaves request policy frozen | Gone | `meerkat/src/service_factory.rs` −23 lines; `meerkat-core/src/agent/state.rs` −243 lines (commit `fdb569aaf`) | factory-creds | Re-verified: no more `hot_swap`/`hotswap` greps. |
| 50 | Live `connection_ref` hot-swap keeps old binding key | Gone | Same commits as #49 | factory-creds | Re-audit once tree rebuilds. |
| 52 | Canonical `SkillKey` refs lowered to `SkillId` | Partial | `meerkat-core/src/agent/runner.rs:990` still constructs `SkillId(format!("{}/{}", key.source_uuid, key.skill_name))` — explicitly lowers structured ref to legacy string; `meerkat-skills/src/source/filesystem.rs:117,311` still uses legacy-ref error & `SkillId(id_str)` path | foundations | The one-line lowering in `runner.rs:990` is the exact pattern the violation describes. Still present. |
| 53 | Source lifecycle status typed then discarded | Partial | `meerkat-core/src/skills/mod.rs:200` (enum `SourceIdentityStatus` still exists); `meerkat-core/src/skills/identity.rs:300,323` — status threads through build() but resolution-time gating still not greppable | foundations | Scan can't confirm resolution-time rejection is wired; marked partial until wave (b) tests it. |
| 55 | Formal composition metadata string-addressed | Survives | `meerkat-machine-schema/src/composition.rs:53,262,286,305,1700,1719,1922,2078,2160` — 9+ `String` fields on producer/consumer/route identity | (none assigned) | Untouched. |
| 56 | Named DSL types erase to String/u64 | Survives | `meerkat-machine-codegen/src/render.rs:3722,4327,4393,4460,4527` — `render_named_type_alias_target(…) == "u64"` lowering still present | (none assigned) | Untouched. |
| 57 | GitHub CI does not gate formal authority governance | Partial | `Makefile:206` adds `rmat-audit` + `audit-generated-headers` to `make ci`; `.github/workflows/ci.yml:387` (gate job) — unchanged | (none assigned) | Makefile added RMAT gate to local `make ci` but GitHub `gate` job still not verified to require it. |
| 58 | Browser/WASM portability enforcement compile-only | Survives | `meerkat-web-runtime/tests/browser_contract.rs:115,279` — tests still `ignored`/no-run; `.github/workflows/ci.yml:349` unchanged | (none assigned) | Untouched. |
| 59 | AuthLeaseHandle docs describe wrong machine owner | Partial | `meerkat-core/src/handles.rs:1,44,84,539,560,635,691,751,975,1003` — 10 `MeerkatMachine` mentions remain; `/docs/reference/architecture.mdx:52` gives `MeerkatMachine` "realtime attachment" (was not the claim); auth-lease doc at line 84 now says "projected from the per-binding AuthMachine" (fixed); comments at 635 still say "MeerkatMachine DSL's" for auth-lease TTL context | (none assigned/foundations?) | Partial: the top-level module doc at line 1 still calls this "MeerkatMachine DSL", and numerous auth-adjacent traits in the file still say "MeerkatMachine DSL". Docs update is half-done. |
| 60 | `auth/profile_test` bypasses canonical provider-resolution | Gone | `meerkat-rpc/src/handlers/auth.rs` −83 lines (commit `4d4450f09`); no `profile_test` greps | rpc-servers | Verified. |
| 61 | Auth invented default realm "dev" + bindings | Gone | `meerkat-rpc/src/handlers/auth.rs` diff; grep for `"dev"` + default realm came up empty | rpc-servers | Verified. Re-audit after rebuild. |
| 62 | CommsTrustReconciler keeps helper-local applied truth | Gone | `meerkat-runtime/src/comms_trust_reconcile.rs` deleted (−600 lines; commit `ce2dbe35e`) | runtime | |
| 63 | Wiring truth bypasses MobMachine | Gone | `meerkat-mob/src/runtime/actor.rs` −1821 lines including `handle_wire`/`handle_unwire`/direct trust mutations; commit chain `0ecd013a7` → `c4566b505` → `0ad584cde` → `e42bdfc44` | mob | Tests still reference old wire/unwire semantics (`meerkat-mob/src/runtime/tests.rs:9483,10298,11577`) — tests now orphaned. |
| 64 | Spawn side effects commit before admission | Partial | `meerkat-mob/src/runtime/actor.rs:1967,3370,3744,3773` still orders provisioning → `MobMachineInput::Spawn` dispatch; the original claim was that trust/roster writes happen before admission. Commit `e77ce8797` says trust-before-admission deleted. | mob | Verify: `handle_spawn_provisioned_batch` at line 3370 — ensure it only commits post-Spawn. |
| 65 | `realtime_attachment_statuses` contract drift | Partial | `sdks/typescript/src/client.ts:1861`, `sdks/python/meerkat/client.py:2084,2093`, `docs/api/rpc.mdx:81,177,249,888,897`, `artifacts/schemas/rpc-methods.json:241` — all still advertise the batch method | rest-mcp-cli-docs | Removed from TS `status`/`submit` but batch-status still intact across wire/schema/docs. |
| 66 | REST dead mob realtime attach/detach endpoints | Gone | `meerkat-rest/src/lib.rs:1216,1220,2025,2039` deleted (commit `488944b7d`); `docs/api/rest.mdx:895` now reads "There is no caller-initiated attach/detach endpoint" | rest-mcp-cli-docs | Verified. |
| 67 | Mobpack web-build bypasses trust/structural validation | Partial | `meerkat-cli/src/main.rs:8447-8482` still hand-parses the pack + computes digest inline; commit `82bb88166` says the bypass was deleted but `WebBuildInvocation` (line 8376) + local TOML derivation path (line 8447) is still the runtime behavior | rest-mcp-cli-docs | Re-audit: did the commit actually route through deploy-time validation, or did it only trim forbidden-capability checks? |
| 68 | Track-B declared authority surface declaration-only | Partial | `meerkat-mob/src/mob_machine.rs:44-57` — `MobMachineCommand::{EnsureMember, Reconcile, ListMembersMatching}` still carry `#[allow(dead_code)]` (pre-existing, not wave-a regression); `WireMembers`/`UnwireMembers`/etc. variants deleted (commit `117a086ff`); new `meerkat-machine-schema/src/catalog/dsl/mob_machine.rs:222-225` DSL copy still declares `BindMemberSession`/`RotateMemberSession`/`ReleaseMemberSession` transitions but no shell path; DSL copy in `meerkat-mob/src/machines/mob_machine.rs:1653-1698` identical | mob | **Critical**: DSL is duplicated between `meerkat-mob/src/machines/mob_machine.rs` and `meerkat-machine-schema/src/catalog/dsl/mob_machine.rs` — the schema catalog copy is stale relative to the mob crate copy on multiple transitions. See Section 2 duplicate-effect table. |
| 69 | Track-B split binding across two effect vocabularies | **Critical straggler** | `meerkat-machine-schema/src/catalog/dsl/mob_machine.rs:222,223,224,259,260,261,314,337,667` still has `MemberSessionBindingSet` / `MemberSessionBindingRotated` / `MemberSessionBindingReleased`; `meerkat-mob/src/machines/mob_machine.rs:1038,1049,1072,1648,1663,1681,1698` (the runtime-used DSL) was collapsed to `MemberSessionBindingChanged` only (commit `62cb8883b`); `meerkat-machine-schema/src/catalog/compositions.rs:320` now uses `MemberSessionBindingChanged` — so the composition driver watches the Changed effect but the schema DSL still emits the old triple | mob | **This is the exact violation the scan described, now split across two DSL files.** `meerkat-machine-schema/src/catalog/dsl/mob_machine.rs` needs the same surgery that `meerkat-mob/src/machines/mob_machine.rs` got. |
| 70 | RecomputeMobPeerOverlayDriver behavior-changing shadow state | Partial (tombstone) | `meerkat-runtime/src/recompute_mob_peer_overlay.rs` deleted (−714 lines); `meerkat-runtime/src/generated/recompute_mob_peer_overlay_driver.rs` deleted (−35 lines); but `meerkat-machine-schema/src/catalog/compositions.rs:305,307` still declares the driver `module_path` pointing at the deleted file; `meerkat-mob/tests/track_b_cutover_source_scan.rs:120,145,151,153` tests still assert the deleted symbols exist | runtime | Compositions catalog still advertises a driver module that no longer exists. Test asserts the absent module. |

### Violations verified **Gone** (no survivors found)

1 (runtime-loop peer-response projection), 2 (runtime-backed turn execution local shadow), 5 (stored-session recovery escape), 7 (recovery re-seeds from driver projection), 9 (provider-native tool defaults pre-resolution), 10 (CredentialSourceSpec::ManagedStore.profile), 11 (default skill-source identity split), 12 (canonical source-identity registry at resolve time), 13 (source precedence by display name), 15 (capability requirements stringly), 16 (SkillId(skill_name) protocol drop), 17 (tool-access denial split), 18 (mob-member WS rotation pinning), 20 (peer-ingress after DSL reject), 21 (supervisor trust before admission rollback), 25 (retire after MobMachine reject continues), 27 (surface-local request-lifecycle authority), 28 (schedule surface reinterprets runtime terminal classes), 31 (session/accept_input untyped), 32 (REST runtime handlers bypass wire contracts — 4 `json!({` lines total vs prior bulk), 33 (CLI realtime `mob_member_target` retired discriminator), 35 (composition-driver layer not execution authority), 36 (composition-driver payload JSON/strings), 37 (overlay driver shadow state — file deleted), 38 (REST runtime surface untyped public methods), 39 (REST fragmented connection_ref — web/TS SDKs cleaned), 46 (AuthProfile.storage inert field — deleted), 48 (persistent lifecycle shadow truth — driver/persistent cleaned), 51 (MCP realtime capabilities ignores WS host — grep quiet), 54 (MCP open_info proxy error flatten — grep clean except stub context).

### Note on state labels

- "Gone" here means the specific symbol/expression the violation fingerprinted is no longer greppable. It does **not** prove the underlying semantic has been re-established through the correct seam. That is wave (b)/(c)'s job.
- Non-compiling tree means we could not validate behavior; every "Gone" above is a syntactic gone, not a behavioral gone.

---

## Section 2 — Patterns the original scan missed

### 2.1 `#[allow(dead_code)]` added since origin/main

`git diff origin/main..HEAD -- '*.rs' | grep "^+.*#\[allow(dead_code)"` returns **zero rows**. No new `#[allow(dead_code)]` was introduced by wave (a).

Pre-existing `#[allow(dead_code)]` in `meerkat-mob/src/mob_machine.rs:44,50,56` (`EnsureMember`/`Reconcile`/`ListMembersMatching`) is intact but was present on `origin/main`. Not a wave-a regression.

### 2.2 `// @generated` on hand-written files

`rg -l "@generated" --type rust` returns only codegen output and `xtask/src/{audit_generated_headers.rs, rmat_audit.rs, rmat_policy.rs, protocol_codegen.rs}` (by-design — these emit or audit the marker). No hand-written files improperly carry `@generated`.

### 2.3 `FIXME|TODO|XXX|HACK` comments added since origin/main referencing dogma/shadow/cutover/follow-up/later

`git diff origin/main..HEAD -- '*.rs' | grep "^+.*(FIXME|TODO|XXX|HACK)"` filtered on `(dogma|shadow|cutover|follow.?up|later|wave|phase)` returns **zero rows**. Wave (a) did not smuggle in new rot markers.

### 2.4 `unimplemented!()` / `todo!()` / `panic!("not implemented"` in non-test code

- `meerkat-core/src/service/mod.rs:1142-1166` — **inside `#[cfg(test)]`**, `UnsupportedSessionService`. Not a violation.
- `xtask/src/rmat_audit.rs:1505,1555,1613,1629,1631,1666,1673,1675` — all inside `#[cfg(test)] mod tests { … }`. Not violations.

No surviving non-test unimplemented/todo/panic("not implemented") in this branch.

### 2.5 `serde_json::Value` fields in semantic seams (untyped passthrough)

| File:Line | Field | Notes |
|---|---|---|
| `meerkat-contracts/src/wire/result.rs:19` | `structured_output: Option<serde_json::Value>` | End-user structured output — pragmatic, likely correct. |
| `meerkat-contracts/src/wire/connection.rs:76` | `options: serde_json::Value` | Provider-specific options bag. Dogma #19 territory. |
| `meerkat-contracts/src/wire/mcp_live.rs:31` | `server_config: serde_json::Value` | MCP passthrough. |
| `meerkat-contracts/src/wire/params.rs:32` | `app_context: Option<serde_json::Value>` | Caller-supplied context. |
| `meerkat-contracts/src/wire/models.rs:35` | `params_schema: serde_json::Value` | JSON Schema. Appropriate. |
| `meerkat-contracts/src/wire/mob.rs:152` | `provider_params: Option<Value>` | **Dogma #9/#12 — turn-override metadata still untyped.** |
| `meerkat-contracts/src/wire/mob.rs:701` | `context: Option<Value>` | Mob work context. |
| `meerkat-contracts/src/wire/runtime.rs:33,45,57` | `result`, `payload`, `result` fields | Runtime command results. |
| `meerkat-contracts/src/wire/runtime.rs:168,170,172,182,184,214` | `policy`, `terminal_outcome`, `durability`, `reconstruction_source`, `persisted_input`, `policy` | **All semantic state carried as untyped Value.** Overlaps violations #31/#38. |
| `meerkat-contracts/src/wire/session.rs:402` | `args: Value` | Tool-call args — pragmatic per design (Box&lt;RawValue&gt; territory). |
| `meerkat-core/src/lifecycle/run_primitive.rs:40` | `Json { value: serde_json::Value }` | ConversationContextAppend variant. |
| `meerkat-core/src/lifecycle/core_executor.rs:42,82` | `CallbackPending { … args: Value }` | Callback routing. |

**Action:** several `runtime.rs` fields at 168–214 are strong candidates for wave (b) typed replacement (dogma #31/#38 territory).

### 2.6 Shadow-state tells in shell modules (`*_cached` / `*_mirror` / `*_shadow` / `*_projection` / `*_snapshot`)

Filtering out DSL projections and generated code, the concerning survivors:

| File:Line | Symbol | Notes |
|---|---|---|
| `meerkat-comms/src/inbox.rs:103,652` | `text_projection: String` | Looks like a stored denormalization. Flag for review. |
| `meerkat-mob/src/roster.rs:508,939,945,951,975,999,1022,1045,1067,1084,1315` | `sync_retiring_projection` | **Dead code tombstone**: 0 external callers. Only self-call + tests. Commit `61eccc095` said "delete sync_retiring_projection_into_roster" but left `sync_retiring_projection` still defined and still called only by tests. |
| `meerkat-mob/src/runtime/roster_authority.rs:20,143,144` | `sync_retiring_projection` trait method | Paired with the dead roster method — trait with no live caller. |
| `meerkat-runtime/src/meerkat_machine/dispatch_ingress.rs:490` | `machine_apply_run_return_projection` | Appears to be DSL-adjacent. Verify owner. |
| `meerkat-runtime/src/meerkat_machine/dispatch_session.rs:263` | `machine_prepare_bindings_projection` | Likely DSL plumbing. |

### 2.7 Duplicate effect vocabulary

**`MemberSessionBindingSet` / `MemberSessionBindingRotated` / `MemberSessionBindingReleased` — the exact triple from violation #72:**

| File:Line | State | Notes |
|---|---|---|
| `meerkat-machine-schema/src/catalog/dsl/mob_machine.rs:222,223,224,259,260,261,314,337,667` | **Still present** | Schema catalog DSL copy untouched. |
| `meerkat-mob/src/machines/mob_machine.rs` (the runtime's actual DSL) | Collapsed to `MemberSessionBindingChanged` only | Commit `62cb8883b` |
| `meerkat-machine-schema/src/catalog/compositions.rs:320` | Uses `MemberSessionBindingChanged` | Matches the runtime-authoritative side. |
| `meerkat-mob/src/machines/mob_machine.rs` stale-comments | Cleaned by commit `d29c8129a` | — |

**This is a canonical duplicate-sources-of-truth violation**: `meerkat-machine-schema/src/catalog/dsl/mob_machine.rs` has diverged from `meerkat-mob/src/machines/mob_machine.rs`. Either the schema catalog is the generator input (in which case the fix was partial) or it's a stale duplicate (in which case it should be deleted). Grep suggests the catalog copy is a **separate DSL compilation unit** registered via `meerkat-machine-schema/src/catalog/dsl/mod.rs:31`.

### 2.8 `.unwrap()` / `.expect()` in RMAT-gated shell seams

In `meerkat/src/service_factory.rs`:
- `:597, :610, :686, :687, :792, :798` — `.expect("capture lock")` / `.expect("probe lock")` — all on `Mutex::lock()` where poisoning is genuinely fatal. Acceptable.

In `meerkat/src/surface/runtime_backed.rs`:
- `:334 .expect("build default persistence")` — **inside a test helper or prod?** Line context suggests `default_runtime_backed_service_from_dir`. Verify.
- `:360, :361, :364, :373, :419, :434, :445, :468` — all in test modules (line 373+ is `tempdir` usage in test).

In `meerkat-mob/src/runtime/handle.rs`:
- `:3442, :3508, :3512, :3524, :3540, :3544` — `serde_json::to_value(&…).expect("…")` — serialization expects. These serialize domain types to JSON for public-facing MCP results. **Borderline**: if the domain types are `Serialize`-derived structs with no non-serde-safe variants, expects are OK. Flag for review.

### 2.9 Tombstones — deleted symbols still referenced

| Symbol | Callers | Break cause |
|---|---|---|
| `canonical_key(…)` function | 6 files: `meerkat-tools/src/builtin/skills/{browse.rs:57,135, load.rs:71, resources.rs:84,130, functions.rs:80}` | commit `4ea425773` deleted the function; callers not updated |
| `recompute_mob_peer_overlay` module | `meerkat-machine-schema/src/catalog/compositions.rs:305,307`; `meerkat-mob/tests/track_b_cutover_source_scan.rs:120,145,151,153` | files at `meerkat-runtime/src/{recompute_mob_peer_overlay.rs, generated/recompute_mob_peer_overlay_driver.rs}` both deleted |
| `mob_member_lifecycle_authority` module | (file deleted — `meerkat-mob/src/runtime/mod.rs` must be updated) | commit `949962121` |
| `sync_retiring_projection` (roster method + trait method) | Only tests + its own tests call it | unused live-code after wave-a intent |

### 2.10 Crates referenced by the dogma catalog but **untouched** this wave

The 70-row scan named the following crates/directories that have **zero LOC delta** in wave (a):

| Crate | Violations the scan assigned to it | Status |
|---|---|---|
| `meerkat-machine-schema/` | #40, #55, #69, #70, #72, #75 | Untouched — duplicate DSL divergence introduced because of this |
| `meerkat-machine-codegen/` | #56, #61, #75, #76 | Untouched |
| `xtask/` | #41, #42, #43, #44, #45, #62 | Untouched |
| `meerkat-machine-kernels/` | #40, #61 | Untouched |
| `meerkat-anthropic/`, `meerkat-gemini/` | #10 | Untouched (though `#10` appears gone from their end via resolver-side deletion) |

---

## Section 3 — LOC delta by crate

Computed from `git diff --numstat origin/main..HEAD`. Crates with net-negative LOC are correct for a delete-only wave; net-positive or zero are flagged.

```
CRATE                                   ADDED   DELETED       NET
meerkat-mob                                42      3086     -3044
meerkat-runtime                            65      2759     -2694
meerkat-rest                                0       940      -940
meerkat-rpc                                 3       887      -884
meerkat                                    45       777      -732
meerkat-core                               15       702      -687
meerkat-session                            19       353      -334
meerkat-skills                              0       288      -288
meerkat-cli                                 0       258      -258
meerkat-mob-mcp                             0       216      -216
meerkat-contracts                           1       187      -186
meerkat-comms                               0       184      -184
meerkat-mob-pack                            0        84       -84
sdks/typescript                             0        80       -80
meerkat-tools                               0        58       -58
meerkat-llm-core                            0        46       -46
meerkat-memory                              0        43       -43
docs                                        0        30       -30
sdks/web                                    2        20       -18
meerkat-mcp-server                          0         4        -4
meerkat-auth-core                           2         3        -1
meerkat-openai                              0         1        -1
```

Observations:
- All touched crates shrank. No net-positive crate.
- **Zero-LOC crates** with assigned violations: `meerkat-machine-schema` (6 violations), `meerkat-machine-codegen` (3 violations), `xtask` (6 violations), `meerkat-machine-kernels` (2 violations), `meerkat-anthropic`, `meerkat-gemini`. See Section 2.10.
- `meerkat-openai` shows −1 line only — its #10 slot (`AuthProfile.storage` handling) should have had matching deletions here if the ManagedStore plumbing was real. The single-line delta suggests the manual-selection logic was already a no-op and only the `pub struct` field removal propagated. Verify the runtime still compiles with a single `AuthMetadata::storage`-shaped concept once the rebuild starts.

---

## Section 4 — Known tombstones from agent reports / commit chain

These are the broken-tree surfaces agents explicitly left for wave (b)/(c). Do **not** treat them as violations to re-delete.

| Commit | Tombstone |
|---|---|
| `4ea425773` | `canonical_key` function removed — 6 callers in `meerkat-tools/src/builtin/skills/*` need a new typed path (dogma #14 replacement). |
| `117a086ff` | `MobMachineCommand::{WireMembers, UnwireMembers, BindMemberSession, RotateMemberSession, ReleaseMemberSession}` variants deleted — DSL still declares the transitions on the `meerkat-mob` side (dogma #71 partial cleanup) and `meerkat-machine-schema` catalog copy is unchanged. Wave (b) needs to either wire the DSL transitions through `MobMachineInput` or finish deleting them from the catalog. |
| `62cb8883b` | Track-B effect vocabulary collapsed to `MemberSessionBindingChanged` on `meerkat-mob` side only — `meerkat-machine-schema/src/catalog/dsl/mob_machine.rs` still has the old triple. |
| `ce2dbe35e` / `f5e366f38` | `CommsTrustReconciler` + `composition_dispatch.rs` + `recompute_mob_peer_overlay*.rs` deleted — the composition catalog still advertises the driver module. |
| `949962121` | `mob_member_lifecycle_authority.rs` file deleted — `meerkat-mob/src/runtime/mod.rs` and any downstream imports need to compile. |
| `0ad584cde` / `e42bdfc44` | Roster wiring seam deleted — `meerkat-mob/src/runtime/tests.rs` + `meerkat-mob/src/runtime/tools.rs` + `meerkat-mob/src/tests/contracts.rs` still reference `wire()`/`unwire()`/bidirectional-trust semantics by doc-comment only. Will break at rebuild. |
| `9bd6a45c6` | Turn metadata override carriers deleted — tombstone is `RuntimeTurnMetadata` itself still defined in `meerkat-core/src/lifecycle/run_primitive.rs`. Either the struct gets simpler or the "single canonical carrier" gets re-established. |
| `fdb569aaf` | Hot-swap auth/turn-limit paths deleted — `meerkat-core/src/agent/state.rs:424,612` still emits `TurnLimitReached`, confirm the handle wiring does not diverge on rebuild. |
| `61eccc095` | `sync_retiring_projection_into_roster` deleted — the inner `sync_retiring_projection` method + trait + tests are live-dead. |

---

## Section 5 — Summary & recommended follow-up deletions

### 5.1 Recommended wave-a-cleanup delete-list

The following are cheap, purely-deletion follow-ups that should ship before wave (b) rebuild begins. File:line references are current HEAD.

| Item | File:Line | Owner |
|---|---|---|
| Dead `sync_retiring_projection` method + trait impl + tests | `meerkat-mob/src/roster.rs:508,930-1315`; `meerkat-mob/src/runtime/roster_authority.rs:20,143-144` | mob |
| Stale `canonical_key(…)` callers (all 6) | `meerkat-tools/src/builtin/skills/browse.rs:57,135`, `load.rs:71`, `resources.rs:84,130`, `functions.rs:80` | foundations |
| Stale composition-driver declaration pointing at deleted module | `meerkat-machine-schema/src/catalog/compositions.rs:295-330` (the `meerkat_mob_seam` `CompositionDriver` block + `module_path`) | runtime |
| Orphaned test asserting deleted overlay driver | `meerkat-mob/tests/track_b_cutover_source_scan.rs:120-160` (delete or rewrite) | mob |
| `meerkat-machine-schema/src/catalog/dsl/mob_machine.rs` duplicate DSL — collapse `MemberSessionBindingSet/Rotated/Released` to `Changed`, mirror the `WireMembers/UnwireMembers/Bind/Rotate/Release` deletions | `meerkat-machine-schema/src/catalog/dsl/mob_machine.rs:222-225,259-261,314,337,667,1612-1698` | mob / runtime (joint) |
| Stale `MeerkatMachine DSL` doc language on auth-lease paths | `meerkat-core/src/handles.rs:1,44,635,641-648,691,751,975,1003` | foundations |
| Docs + schema cleanup for retired SDK surface | `docs/api/rpc.mdx:79,82,168,249,888,897`; `artifacts/schemas/rpc-methods.json:241`; Python SDK `sdks/python/meerkat/client.py:2071,2149` (`status`/`submit`) | rest-mcp-cli-docs |
| Stale `SkillKey` → `SkillId` lowering | `meerkat-core/src/agent/runner.rs:990` | foundations |
| Stale legacy-skill-ref format parsing | `meerkat-skills/src/source/filesystem.rs:117,311` | foundations |
| REST GET endpoints still calling `ensure_runtime_session_registered` | `meerkat-rest/src/lib.rs:1280,1915,1986,2663,3495` (at least 3495 is read-only) | rest-mcp-cli-docs |
| REST residual `json!({…})` hand-construction | `meerkat-rest/src/lib.rs` (4 sites, see §2.5) | rest-mcp-cli-docs |

### 5.2 Untouched-crate follow-up

`meerkat-machine-schema`, `meerkat-machine-codegen`, `xtask`, `meerkat-machine-kernels` together carry dogma violations #40, #41, #42, #43, #44, #45, #55, #56, #62 (CI gate), #75, #76. **None** of these were touched in wave (a). A separate "wave-a-machine-substrate" cleanup is needed, or these 11 violations must be explicitly re-queued for wave (b)/(c).

### 5.3 Overall audit verdict

Wave (a) achieved the primary goal — **bulk deletion of shell-owned dogma in the runtime/mob/rest/rpc/surface crates (~10,000 LOC removed across ~90 files)**. The major structural violations (#62 CommsTrustReconciler, #63 wiring bypass, #35/#36/#37 composition-driver layer, #27 request-lifecycle surface, #38 REST runtime surface) are genuinely gone.

Residual debt falls in three buckets:

1. **Tombstones (Section 4)** — agents deleted symbols whose callers remained. Expected for a broken-tree delete wave; wave (b) absorbs.
2. **Duplicate-source divergence** (dogma #69 + the broader `meerkat-machine-schema/src/catalog/dsl/mob_machine.rs` duplication). Needs a single owner decision: either delete the schema copy as duplicate, or regenerate both from the catalog source.
3. **Untouched crates** (`meerkat-machine-schema`, `xtask`, codegen, kernels). 11 violations sat outside any agent's scope. Flag for wave (b) intake.

No forbidden patterns were smuggled in: zero new `#[allow(dead_code)]`, zero new dogma-tagged TODOs, zero non-test `unimplemented!()`/`todo!()`, zero crates with net-positive LOC.
