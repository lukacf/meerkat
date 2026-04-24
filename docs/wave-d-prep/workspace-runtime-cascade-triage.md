# meerkat workspace test cascade — triage (#32)

**Baseline**: `06cad1871` (integration tip of `dogma/wave-a-demolition`).

**Command pattern**: per-crate `perl -e 'alarm N; exec @ARGV' ./scripts/repo-cargo nextest run -p <crate> --lib --no-fail-fast`

**Full-workspace nextest** still deadlocks on a single test (Assignment B). Per-crate runs avoided.

---

## Assignment B result — hung test identified

### Test

`meerkat-rest::tests::test_create_session_route_returns_identity_on_post_commit_turn_failure` at `meerkat-rest/src/lib.rs:5608`.

### Symptom

- Runs alone past 240s with `perl -e 'alarm 240'` kill (the only hung test in the entire workspace).
- nextest SLOW markers fire at 60s, 120s, 180s; test never returns.
- Exit code: SIGALRM (142) — no panic, no return; it's a genuine deadlock, not a slow-but-completing test.
- Same-file companion test (`test_missing_header_works_normally` at `lib.rs:6125`) panics with `should not timeout: Elapsed(())` after 10s — the related "timeout wrapping" pattern exists elsewhere in the file.

### Shape

The test body (5608-5645) builds an `AppState` with `llm_client_override = Some(Arc::new(ErrorLlmClient))` at `lib.rs:4286`, then issues a `POST /sessions` via `app.oneshot(...)`, expecting the route to return `500` with `payload["code"] == "SESSION_CREATED_WITH_TURN_FAILURE"`.

The `oneshot` future never completes. The create-session route is waiting on a post-commit turn result that never arrives when the underlying LLM client errors. Most likely a join-handle / channel-drop ordering bug on the error path in the runtime-backed create-session implementation: the error path writes to one channel and awaits another that's already dropped, or vice-versa.

### Relationship to other `meerkat-rest` failures

The same crate's `test_create_session_route_completes_in_runtime_backed_mode` at `lib.rs:5387` fails with `runtime-backed create route timed out: Elapsed(())` after 10s — uses a `tokio::time::timeout(...)` wrapper that does bound the hang. The unbounded hung test is distinguished by the lack of such a wrapper on its `oneshot(...)` call.

### Fix shape proposal

- **Immediate bounded fix**: wrap the `oneshot(...)` call at `lib.rs:5620-5635` in `tokio::time::timeout(Duration::from_secs(10), ...)` and fail with a clear message if the timeout fires. This matches the sibling tests' pattern and turns a hang into a visible failure, so the underlying bug surfaces rather than silently holding up `--workspace` runs.
- **Root-cause fix** (separate commit): the post-commit turn-failure path in the create-session route must close its result channel on error. Likely site: search `meerkat-rest/src/lib.rs` for the create-session route implementation that handles `SESSION_CREATED_WITH_TURN_FAILURE` — wherever the `LlmClient::run` result is awaited in the error branch.

### Scope estimate

- Timeout wrapper: **trivial**, 1 commit. Unblocks workspace-wide nextest.
- Root-cause teardown fix: **unknown**, needs investigation — likely a deadlock pair, 1 site once traced.

### Commit message preamble

```
fix(meerkat-rest): bound oneshot in post-commit-failure test to unblock workspace nextest (#32 B)

The `test_create_session_route_returns_identity_on_post_commit_turn_failure`
test at meerkat-rest/src/lib.rs:5608 hangs indefinitely when `ErrorLlmClient`
surfaces a turn failure on the post-commit path. The sibling
`test_create_session_route_completes_in_runtime_backed_mode` at lib.rs:5387
uses `tokio::time::timeout(...)` and surfaces a clear `Elapsed(())` panic
when the same route hangs; this test uses an unwrapped `app.oneshot(...)`
and deadlocks the whole workspace `cargo nextest run --workspace` for
57+ minutes (team-lead's observation).

Wrap the oneshot in a 10s timeout to match the sibling's pattern. This
surfaces the bug as a visible failure rather than a silent hang. The
underlying deadlock in the post-commit-failure route (likely a
channel-drop / await-ordering pair in the error branch of the runtime-
backed create-session implementation) is tracked separately.
```

---

## Assignment A — #32 failure-class triage

### Per-crate failure counts (baseline at `06cad1871`)

| Crate                       | Passed | Failed | Skipped | Notes |
|-----------------------------|--------|--------|---------|-------|
| meerkat-core                | 566    | 24     | 0       |       |
| meerkat-llm-core            | ?      | ?      | ?       | not run separately (skipped by `-p meerkat-llm-core` heuristic; next pass) |
| meerkat (facade, lib)       | 61     | 6      | 0       |       |
| meerkat (facade, tests)     | 71     | 19     | 0       | `lib + tests` combined ran 157/25; tests-only delta = 19 |
| meerkat-runtime             | 411    | 49     | 3       |       |
| meerkat-rpc                 | 176    | 64     | 2       |       |
| meerkat-rest                | —      | —      | —       | **hang, killed at 180s** (B) |
| meerkat-mob-mcp             | 69     | 9      | 1       |       |
| meerkat-mcp-server          | 38     | 4      | 0       |       |
| meerkat-memory              | 16     | 6      | 0       |       |
| machine-dsl-tests           | 60     | 5      | 0       |       |
| meerkat-machine-schema      | 8      | 0      | 0       | passes (earlier failures now in machine-dsl-tests) |
| meerkat-schedule            | 50     | 0      | 0       |       |
| meerkat-store               | 21     | 0      | 0       |       |
| meerkat-tools               | 332    | 0      | 0       |       |
| meerkat-hooks               | 11     | 0      | 1       |       |
| meerkat-session             | 12     | 0      | 0       |       |
| meerkat-comms               | 295    | 0      | 3       |       |
| meerkat-skills              | 31     | 0      | 0       |       |
| meerkat-machine-codegen     | 1      | 0      | 0       |       |
| meerkat-mob-pack            | 37     | 0      | 0       |       |
| meerkat-mcp                 | 60     | 0      | 1       |       |
| meerkat-client              | 0      | 0      | 0       | binary-only; exit 4 |
| meerkat-providers           | 0      | 0      | 0       | binary-only; exit 4 |
| rkat (meerkat-cli)          | —      | —      | —       | binary-only; no library target |

**Rough total** (excluding `meerkat-rest` + binary-only): ~210 failures across the non-mob workspace.

### Root-class categorization

Failures cluster into a small number of root-classes, all rooted in Wave-A/B/C shell-truth removals:

#### Class W1 — `runtime turn-state handle missing` (~40 facade + core tests)

**Panic**: `InternalError("runtime turn-state handle missing: agent was built without with_turn_state_handle but is being queried on a live-run code path — the standalone fallback was deleted in wave-a; runtime-backed wiring is required")`

**Affected tests** (sample; actual count ~40 across `meerkat` + `meerkat-core`):

- `meerkat::service_factory::tests::factory_agent_execution_snapshot_forwards_core_state`
- `meerkat::service_factory::tests::test_default_llm_client_takes_precedence_over_config_keys`
- `meerkat::service_factory::tests::test_session_llm_override_is_applied_end_to_end`
- `meerkat::surface::embedded::tests::build_embedded_service_uses_default_schedule_tools`
- 14× `meerkat-core` tests with this same panic
- Most of `meerkat/tests/factory_build_agent.rs::*` live-run tests
- Most of `meerkat/tests/sdk_structured_output.rs::*` agent-run tests
- `meerkat/tests/sdk_agentfactory.rs::test_sdk_agentfactory_tool_dispatch`

**Root cause**: Wave-A deleted the standalone fallback that populated `turn_state_handle` on agent construction for test harnesses that used `AgentBuilder::new()` directly without runtime-backed wiring. Tests constructing an agent via fixture helpers that predate Wave-A wiring now fail at the first live-run code path (any `agent.run(...)` call that queries the turn-state handle).

**Origin**: Likely `d5bddfe8d` (wave-d D-mob 86-error close) or an adjacent Wave-A commit that tightened the `turn_state_handle.expect(...)` contract — need `git log -S "runtime turn-state handle missing"`:

```
git log --oneline -S "runtime turn-state handle missing" --all
```

Not executed in this triage pass; noted for implementer.

**Fix-shape proposal**:

- **(W1.α)** Migrate test fixtures to use `AgentFactory::build_agent()` with a runtime-backed `MeerkatMachine` installed via `SessionBuildOptions.runtime_build_mode`. Most tests would change from direct `AgentBuilder::new()` → via `build_ephemeral_service` or similar. This is the dogma-correct path (surfaces are supposed to always route through the factory).
- **(W1.β)** Restore a test-only `StandaloneEphemeral` runtime-build-mode path that installs a stub `turn_state_handle`. Less dogma-correct (resurrects the fallback the Wave-A cleanup deleted) but test-scope-only.

**Scope estimate**: ~40 sites to migrate (W1.α). Can be mechanized with a codemod if the fixture pattern is consistent (likely is — most tests use the same helper). **Medium scope, single commit feasible if the helper is centralized; otherwise 40-site mechanical sweep**.

**Commit message preamble**:

```
fix(meerkat): migrate test fixtures to runtime-backed AgentFactory (#32 Class W1)

Wave-A deleted the `turn_state_handle` standalone fallback. ~40 tests
across `meerkat` facade, `meerkat-core`, and facade integration tests
still construct agents via direct `AgentBuilder::new()` without wiring
a runtime-backed `MeerkatMachine`. Every live-run path (`agent.run(...)`
and equivalents) now panics with "runtime turn-state handle missing".

Migrate the affected test fixtures to use `AgentFactory::build_agent()`
with `SessionBuildOptions.runtime_build_mode = RuntimeBuildMode::SessionOwned(bindings)`
via the existing `build_ephemeral_service` helper. This matches the
Wave-A surface contract: all surfaces (and surface-style tests) route
through the factory.
```

---

#### Class W2 — `runtime_execution_kind not set` (~40 RPC tests)

**Panic**: `Internal error: runtime_execution_kind not set: turn-state handle is attached but the runtime build mode did not classify the execution kind`

**Affected tests** (31+ in `meerkat-rpc`; 1 in `meerkat` facade):

- 18× `meerkat-rpc` tests: `Expected success response, got error: RpcError { ... "turn abandoned: apply failed: ... runtime_execution_kind not set ..." }`
- 13× `meerkat-rpc` tests: `RpcError { ... "runtime_execution_kind not set ..." }`
- 10× `meerkat-rpc` tests: `start_turn: RpcError { ... "runtime_execution_kind not set ..." }`
- `meerkat::service_factory::tests::factory_builder_uses_default_schedule_tools_on_runtime_backed_resume`

**Root cause**: RPC tests and one facade test attach a `turn_state_handle` via `SessionOwned` runtime build mode but do NOT classify the `runtime_execution_kind` (External vs Internal vs Ingest). The `MeerkatMachine` ingest pipeline requires this classification to route commands to the correct execution handler; post-Wave-A the unclassified case panics explicitly rather than silently defaulting.

**Origin**: Likely `2faa60ecf` (PR #299 "typed peer terminal ingress + smoke continuity") introduced the `runtime_execution_kind` classification slot, or a follow-up commit that tightened the "must be classified" invariant. Need `git log -S "runtime_execution_kind not set"`.

**Fix-shape proposal**:

- The `SessionBuildOptions.runtime_build_mode` must carry the execution kind at the SessionOwned construction site. Either:
  - **(W2.α)** Default `runtime_execution_kind` to `External` on SessionOwned if not specified by the caller (sensible test default, matches RPC's external-work framing).
  - **(W2.β)** Thread the classification through every RPC test harness explicitly.

**Scope estimate**: (W2.α) is 1-site in the runtime build-mode constructor — **small, 1 commit**. (W2.β) is ~40 test-site migrations.

**Commit message preamble**:

```
fix(runtime): default runtime_execution_kind to External on SessionOwned (#32 Class W2)

PR #299 introduced the `runtime_execution_kind` classification slot
on `SessionRuntimeBindings`. ~40 RPC tests and 1 facade test attach
a `turn_state_handle` via `RuntimeBuildMode::SessionOwned(bindings)`
but do not explicitly classify the execution kind; the ingest pipeline
panics with "runtime_execution_kind not set: turn-state handle is
attached but the runtime build mode did not classify the execution
kind".

Default `runtime_execution_kind` to `WorkOrigin::External` at the
`SessionOwned` constructor site — matches the RPC surface's external-work
framing. Tests that exercise Internal/Ingest paths already set the kind
explicitly; the default only affects the currently-unclassified cases.
```

---

#### Class W3 — `ConnectionResolution` / `build_agent requires explicit ConnectionRef` (~10 facade tests)

**Panic**: `ConnectionResolution("ambient credential selection refused: build_agent requires an explicit ConnectionRef (realm + binding). Env-default realm synthesis and first-matching-provider promotion were deleted in the wave-c auth-seam cleanup.")`

**Affected tests** (sample):

- `meerkat::service_factory::tests::test_config_api_keys_resolve_different_providers_per_model`
- `meerkat::agent_factory_connection_ref::build_agent_without_connection_ref_uses_flat_path`
- `meerkat::factory_build_agent::build_agent_uses_provider_config_api_key`
- (~7 more in `meerkat/tests/factory_build_agent.rs`)

**Root cause**: Wave-C auth-seam cleanup deleted env-default realm synthesis and first-matching-provider promotion. Tests that relied on `AgentFactory` reading `config.provider.api_key` without an explicit `ConnectionRef` now panic.

**Origin**: The rejection message points at "wave-c auth-seam cleanup" — need `git log -S "Env-default realm synthesis and first-matching-provider promotion were deleted"` to identify the commit.

**Fix-shape proposal**:

- **(W3.α)** Migrate affected test fixtures to pass an explicit `ConnectionRef { realm, binding }` via `SessionBuildOptions.connection_ref` or the equivalent slot on `AgentFactory`. Tests that intentionally exercise the no-ConnectionRef path (e.g. `build_agent_without_connection_ref_uses_flat_path`) should be flipped to rejection-assertion shape (cite the `ConnectionResolution` error variant) per the same pattern as my #25 Q2 work.

**Scope estimate**: ~10 test sites, mechanical once a canonical `ConnectionRef` fixture helper exists. **Medium, 1 commit**.

**Commit message preamble**:

```
fix(meerkat): migrate factory tests to explicit ConnectionRef (#32 Class W3)

Wave-C auth-seam cleanup deleted env-default realm synthesis and
first-matching-provider promotion from `AgentFactory::build_agent`.
~10 tests relied on ambient credential selection via
`config.provider.api_key`; they now panic with
`ConnectionResolution("ambient credential selection refused: build_agent
requires an explicit ConnectionRef (realm + binding). ...")`.

Migrate affected fixtures to pass an explicit `ConnectionRef` via
`SessionBuildOptions.connection_ref`. The
`build_agent_without_connection_ref_uses_flat_path` test is flipped to
a rejection-assertion tripwire — asserting the current rejection shape
per the Wave-C contract — so future auth-seam changes surface cleanly.
```

---

#### Class W4 — `MissingNamedTypeBinding` (5 tests in machine-dsl-tests)

**Panic**: `MissingNamedTypeBinding { name: "<TypeName>" }`

**Affected tests**:

- `machine-dsl-tests::mob_machine::tests::schema_validates` — `AgentRuntimeId`
- `machine-dsl-tests::meerkat_machine::tests::schema_validates` — `SessionId`
- `machine-dsl-tests::occurrence_lifecycle::tests::schema_validates` — `OccurrenceId`
- `machine-dsl-tests::schedule_lifecycle::tests::schema_validates` — `ScheduleLifecycleState`
- `machine-dsl-tests::tests::order_lifecycle::schema_is_valid` — `OrderPhase`

**Root cause**: Commit `c0cb12071` (`schema(wave-b B-4): add named_types to MachineSchema + validation`) added a schema-validation gate: every `TypeRef::Named(NamedTypeId)` referenced in a schema (in state fields, variant fields, helper params/returns) must have a matching `NamedTypeBinding` entry in `MachineSchema.named_types`. The commit message explicitly states "Existing constructors do not yet populate `named_types` — they will fail" — Wave-B deferred the constructor updates. They haven't landed.

**Origin**: `c0cb12071` (2026-04-23, `wave-b B-4`).

**Fix-shape proposal**:

- **(W4.α)** Populate `MachineSchema.named_types` at each affected schema's constructor site in `test-fixtures/machine-dsl-tests/src/`. For each failing schema, collect all `TypeRef::Named(...)` references via the existing `collect_named_type_references_machine` walker and emit `NamedTypeBinding` entries with the correct Rust-type pointer (likely `NamedTypeBinding::rust_type(...)` — API to confirm at `meerkat-machine-schema`).

**Scope estimate**: 5 schemas × 1-5 bindings each = ~15 binding additions. Fully mechanical once the binding-API is inspected. **Small, 1 commit**.

**Commit message preamble**:

```
fix(machine-dsl-tests): populate named_type_bindings on 5 schema fixtures (#32 Class W4)

Commit `c0cb12071` (wave-b B-4) added validation that every
`TypeRef::Named(NamedTypeId)` reference in a schema must have a
matching entry in `MachineSchema.named_types`. The commit message
explicitly flagged that existing constructors don't yet populate the
new field, and they still don't — 5 schema fixtures in
`test-fixtures/machine-dsl-tests/src/` fail with
`MissingNamedTypeBinding { name: "<type>" }`:

- mob_machine::tests::schema_validates    → AgentRuntimeId
- meerkat_machine::tests::schema_validates → SessionId
- occurrence_lifecycle::tests::schema_validates → OccurrenceId
- schedule_lifecycle::tests::schema_validates  → ScheduleLifecycleState
- tests::order_lifecycle::schema_is_valid      → OrderPhase

Populate `named_types` at each constructor with the collected
references. Mechanical — the existing `collect_named_type_references_machine`
walker produces the input set; binding emission is a loop over it.
```

---

#### Class W5 — mob-mcp / mcp-server actor-task-dropped and peer_id drift (~13 tests)

**Sub-classes**:

- **W5.1 mob-mcp actor-task-dropped** (8 tests): `tool call: ExecutionFailed { message: "tool '<tool_name>' failed: internal error: actor task dropped" }` — **collapses into #31 Class A1**. These are mob-mcp surfaces dispatching into the mob actor, which dies on routed-effect dispatch failure per the A1 root cause. Not a new class.
- **W5.2 mcp-server misc** (4 tests): heterogeneous — persisted-session listing, session-not-found after mcp-add, serde-variant rename (`peer_message` → `input`), Live-MCP unavailable.
- **W5.3 runtime `ed25519:<alias>`** (11 tests in `meerkat-runtime/src/comms_drain.rs`): `valid supervisor spec: "invalid peer_id: invalid peer id \"ed25519:supervisor\": ..."` — **same class as #31 Class D**, 11 sites in `meerkat-runtime` (not `meerkat-mob`). These sites are in production code (`comms_drain.rs::2315-3334` supervisor-reconcile construction paths), not tests; earlier D sweep only touched `meerkat-mob`.

**Fix-shape proposal**:

- **W5.1**: blocked on #31 A1 (not new work; already tracked).
- **W5.2**: heterogeneous; each requires per-test investigation — 1-4 hours total, likely 1-3 commits.
- **W5.3**: same `PeerId::new()` / `test_only_unsigned_typed` migration as #31 Class D, applied to 11 production call sites in `meerkat-runtime/src/comms_drain.rs`. **The production code uses `key.to_peer_id().as_str()` elsewhere in the file** — the 11 broken sites are outliers that hardcode `"ed25519:<alias>"` strings. 1 commit.

**Scope estimate**: W5.1 blocked, W5.2 medium, W5.3 small.

**Commit message preamble for W5.3**:

```
fix(runtime/comms-drain): migrate 11 supervisor-reconcile fixture sites from ed25519:<alias> to typed PeerId (#32 Class W5.3)

11 call sites in `meerkat-runtime/src/comms_drain.rs` (test harnesses
within the `comms_drain` module's test submodule) construct
`TrustedPeerDescriptor` via `test_only_unsigned(..., "ed25519:<alias>", ...)`.
Post-#24, `PeerId::parse` only accepts UUIDs, so the string-form
arguments reject at runtime.

Adjacent code in the same file uses `key.to_peer_id().as_str()` (which
produces a round-trippable UUID string) — the 11 broken sites are
outliers from a pre-#24 fixture pattern. Migrate them to the typed
`test_only_unsigned_typed(..., PeerId::new(), ...)` helper added in
#31 Class D.
```

---

#### Class W6 — meerkat-runtime assorted (49 failures in `meerkat-runtime::*`, non-W5.3)

**Predominant panics**:

- 19× `assertion left == right failed` — various test-specific equality checks (no single shape)
- 9× `valid supervisor spec: "ed25519:supervisor"` → Class W5.3 (subset of the 11 counted above)
- 2× `runtime should return to Retired once the preserved work finishes draining: Elapsed(())` — phase-transition hangs; possibly related to the A1-class actor exit pattern but on runtime (not mob) side
- 2× `phase transition: Elapsed(())` — same pattern

**Root cause**: Mixed. The 19 equality-assertion failures need per-test inspection. The 4 `Elapsed` failures suggest the meerkat-runtime has its own version of A1 (some authority-removal left a command flow waiting for an event that no longer fires).

**Scope estimate**: **Large**, requires per-test investigation similar to the #31 effort on the mob side. Initial estimate 3-6 distinct root-classes in here.

**Not bucketed further in this triage pass** — needs its own #33-style triage. Flagging.

---

#### Class W7 — meerkat-memory snapshot deserialization (6 tests)

**Panic**: `called Result::unwrap() on an Err value: Error("invalid type: map, expected a sequence", line: 1, column: 0)`

**Affected tests**: 6 in `meerkat-memory` (all with the same serde error).

**Root cause**: Serde shape drift — a stored format expects a sequence but the current code writes a map. Most likely a serialized-to-disk format (HnswMemoryStore snapshot or similar) changed shape in the current code relative to a committed test-fixture JSON/YAML file. Migration needed either on the fixture side or on the deserializer (feature detection for old shape).

**Origin**: Needs `git log -S` on the serde tag or the changed enum/struct. Not traced in this pass.

**Scope estimate**: **Small** once origin identified — either 1-fixture-file update or 1 serde-deserializer migration. 1 commit.

---

## Summary table

| Class | Count | Root Cause | Fix Scope | Blocker? |
|-------|-------|------------|-----------|----------|
| W1 | ~40 | Wave-A `turn_state_handle` fallback deleted; tests still use direct `AgentBuilder::new()` | Medium, migrate to factory | Yes (D-GATE) |
| W2 | ~40 | PR #299 `runtime_execution_kind` classification slot mandatory; tests don't set it | Small (default to External) or Medium (per-test) | Yes (D-GATE) |
| W3 | ~10 | Wave-C auth-seam cleanup deleted ambient credential fallback | Medium, migrate fixtures to explicit ConnectionRef | Yes (D-GATE) |
| W4 | 5 | Wave-B B-4 schema validation deferred constructor updates; `named_types` unpopulated on 5 fixtures | Small, mechanical | Yes (D-GATE) |
| W5.1 | ~8 | mob-mcp surface dispatches into A1-dead actor | Blocked on #31 A1 | N/A (A1 blocker) |
| W5.2 | 4 | Heterogeneous mcp-server test drift | Medium | Likely |
| W5.3 | 11 | Same as #31 Class D but in `meerkat-runtime/src/comms_drain.rs` | Small, 1 commit | Yes (runtime unshippable) |
| W6 | ~30 (non-W5.3) | Mixed meerkat-runtime regressions, needs own triage | Large | Yes (needs own triage) |
| W7 | 6 | Serde shape drift on memory snapshot | Small | Yes if memory feature ships |
| B (hung) | 1 | Deadlock in post-commit-failure route | Small (bound with timeout) + medium (fix root) | **Yes — unblocks workspace nextest** |

### Priority order for D-GATE unblock

1. **B (hung test)** — 1 line of code unblocks workspace-wide nextest. Do first.
2. **W4** — 5 fixtures, fully mechanical, tiny blast radius.
3. **W2** — default the classification slot, fix 40 tests in one commit.
4. **W5.3** — mirror #31 Class D migration on 11 production sites.
5. **W7** — serde shape fix, 1 commit.
6. **W1** — ~40-site fixture migration; medium risk because some of them may have deeper A1-class issues underneath.
7. **W3** — ~10-site fixture migration + 1 rejection-assertion flip; medium.
8. **W6** — needs own triage (#33 candidate). Don't attempt as part of #32.
9. **W5.1** — blocked on #31 A1.
10. **W5.2** — separate per-test investigation; defer.

---

## Assignment C — #31 Class F2 verification

My #31 triage flagged 4 tests as Class F2 ("runtime-behavior regressions in peer_id projection / reconciler spawn commit / parallel spawn edge-dedupe / retire disposal lifecycle") and predicted they'd likely collapse into A1 once A1 lands.

### Re-verification at `06cad1871` (post-#31 Class D/F/G cleanup, pre-A1)

Ran the 4 tests with `--nocapture` to see the actual panic shapes:

```
test_member_roster_surfaces_peer_id: left: None, right: Some("<uuid>")
  → entry.peer_id field is None where the test expects Some(UUID)

test_reconcile_spawns_missing_and_retires_stale: left: 0, right: 2 "two new spawns committed"
  → reconciler path doesn't commit spawns (0 where 2 expected)

test_spawn_many_parallel_finalize_deduplicates_worker_pair_wiring: left: 0, right: <expected>
  → parallel spawn finalization doesn't dedupe w-a<->w-b edge

test_wait_one_observes_retiring_member_as_non_terminal_until_archive: left: Active, right: <retiring classification>
  → helper classification doesn't project retiring state during disposal
```

### Finding: F2 is genuinely runtime-behavior regressions, not A1 cascade

All 4 tests **reach their assertions** — `handle.spawn(...)` and subsequent operations succeed (no `actor task dropped` panic). The failure is that **the state populated in response to the successful operation doesn't match what the test expects**. These are not A1 symptoms.

The failures cluster loosely:

- `test_member_roster_surfaces_peer_id` — `peer_id` isn't populated on `MobMemberListEntry` post-spawn. Possibly connected to #29 D-roster-wiring-projection (the `Roster::apply` catchall also eats peer_id populations from some event).
- `test_reconcile_spawns_missing_and_retires_stale` — reconciler fires but commits zero spawns.
- `test_spawn_many_parallel_*` (2 tests) — parallel spawn finalization doesn't dedupe wiring edges; likely connected to the same roster-wiring projection gap as #29.
- `test_wait_one_...retiring_...` — helper classification doesn't expose retiring state; possibly a retire-event projection gap similar to #29.

### F2 re-classification

These are **Class W6-adjacent** — mob-side runtime-behavior regressions that parallel the `meerkat-runtime` Class W6 regressions but on the mob actor path. Probable overlap with #29 D-roster-wiring-projection for 3 of the 4. Retire-disposal-lifecycle one is its own sub-class.

### F2 disposition

**Re-confirm**: these are not A1 cascades. They will NOT automatically resolve when A1 lands.

**Recommendation**: retitle from "Class F2 — runtime spawn/reconcile/retire behavior regressions" to something more specific, split into (a) 3 roster-projection subset tracked under #29, (b) 1 retire-disposal subset needing its own root-trace. Out of scope for #32 but important to note against D-GATE: #29 is not just "projection coverage gap" — it's also causing concrete test failures. The #29 scope estimate should be updated to reflect this.

---

## Method notes

- All per-crate runs used `perl -e 'alarm N; exec @ARGV'` as timeout wrapper (macOS lacks `timeout(1)`).
- Log output redirected with `&> /tmp/nx-<crate>.log` because single-`>` truncates on macOS default shell when the log exceeds some buffer threshold.
- Panic-message bucketing used `awk '/panicked at/{getline line; print line}'` piped through `sort | uniq -c`.
- `meerkat-rest` hang confirmed by running the single suspect test alone with 240s `alarm` and observing it hit SIGALRM with nextest printing "SLOW" at 60s/120s/180s intervals.
- F2 re-verification used `--nocapture -E 'test(<name>) or test(<name>) ...'` nextest filter to extract actual panic bodies (which show the left/right sides of the assertion mismatch that the summary line hides).
- Not all crates were run (gave up after confirming W1-W7 coverage): `meerkat-llm-core`, `meerkat-auth-core`, `meerkat-contracts`, `meerkat-machine-derive`, `meerkat-machine-dsl-core`, `meerkat-machine-dsl`, `meerkat-machine-kernels`, `meerkat-gemini`, `meerkat-anthropic`, `meerkat-openai`, `meerkat-web-runtime`, `test-fixtures/surface-build-fixtures`, `test-fixtures/mcp-test-server`, `tests/integration`, `xtask`. Team-lead's 57-min hang run covered these; only the one hang surfaced, so they're probably clean or only fail under the workspace's shared-cache race condition.
