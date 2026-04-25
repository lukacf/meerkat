# Wave-c persistence migration (v0 → v1)

Scope: design only. Covers the v0→v1 migration induced by wave-b retypes
(`RuntimeTurnMetadata` V3, `WireInputState` typed fields, `ConnectionRef`
structural). Implementation lands in wave-c.

## 1. Persisted-row inventory

Three physically distinct stores carry serialized shapes that wave-b
touches, plus the wire-only envelope that rides on top of RPC.

### 1.1 `sessions` table (SQLite) — `SessionStore` blob row
File: `meerkat-store/src/sqlite_store.rs:15-24` (DDL) and
`meerkat-store/src/sqlite_store.rs:71-108` (writer).

```
sessions(session_id, created_at_ms, updated_at_ms, message_count,
         total_tokens, metadata_json TEXT, session_json BLOB)
```

`session_json` is `serde_json::to_vec(&Session)` of
`meerkat-core/src/session.rs:32-47`. `Session` already carries
`version: u32` (`SESSION_VERSION = 1` — `meerkat-core/src/session.rs:26`),
with a `#[serde(default = "default_version")]` so pre-versioned rows
deserialize as v1. `Session.metadata: serde_json::Map<String,Value>` holds
`SESSION_METADATA_KEY → SessionMetadata` (`session.rs:696-711`,
`session.rs:852-892`).

Fields changed by wave-b inside `SessionMetadata`:
- `provider_params: Option<serde_json::Value>` → `Option<ProviderParamsOverride>` (`session.rs:861`; commit `197a70b4a`)
- `connection_ref: Option<ConnectionRef>` — *structural change*, same field name; v0 inner shape is `{realm_id: String, binding_id: String, profile: Option<String>}`, v1 is `{realm: RealmId, binding: BindingId, profile: Option<ProfileId>}` (`session.rs:891`; commit `cf90208b7`, `meerkat-core/src/connection.rs:86-106`)
- `realm_id: Option<String>` (session.rs:871) and the string `realm_id`/`binding_id` pair inside v0 `ConnectionRef` — both collapse under v1 `RealmId`/`BindingId` newtypes
- `SessionLlmIdentity.provider_params: Option<Value>` (`session.rs:907`) and `SessionLlmIdentity.connection_ref` (`session.rs:921`) — same pair of changes, projected through hot-swap

The secondary `metadata_json` column is a denormalized copy of
`session.metadata` used only for listing (`sqlite_store.rs:76,188-214`);
fields here are the same `Map<String,Value>`, not the typed struct.

### 1.2 `runtime_*` tables — `RuntimeStore`
File: `meerkat-runtime/src/store/sqlite.rs:18-43` (DDL).

```
runtime_input_states(runtime_id, input_id, state_json BLOB)
runtime_boundary_receipts(runtime_id, run_id, sequence, receipt_json)
runtime_session_snapshots(runtime_id, session_snapshot BLOB)
runtime_states(runtime_id, runtime_state_json)
runtime_ops_lifecycle(runtime_id, state_json)
```

- `runtime_input_states.state_json` = `StoredInputState` via the custom
  `InputStateSerde` helper (`meerkat-runtime/src/input_state.rs:257-336`).
  `persisted_input: Option<Input>` (`input_state.rs:278`) recursively
  carries `PromptInput`/`PeerInput`/`FlowStepInput`/... each of which
  owns `turn_metadata: Option<RuntimeTurnMetadata>`
  (`meerkat-runtime/src/input.rs:265,291,409`). Every field retyped in
  B-6 lives under here when the input was persisted:
  - `RuntimeTurnMetadata.provider_params` — `Option<Value>` → `Option<ProviderParamsOverride>` (`run_primitive.rs:292`)
  - `RuntimeTurnMetadata.model: Option<String>` → `Option<ModelId>` (`run_primitive.rs:286`)
  - `RuntimeTurnMetadata.provider: Option<String>` → `Option<Provider>` (`run_primitive.rs:289`)
  - `RuntimeTurnMetadata.additional_instructions: Option<Vec<String>>` → `Option<Vec<TurnInstruction>>` (`run_primitive.rs:283`)
  - NEW fields: `connection_ref: Option<ConnectionRef>` (`run_primitive.rs:295`), `keep_alive: Option<KeepAlivePolicy>` (`run_primitive.rs:298`)
- `runtime_session_snapshots.session_snapshot` is a second serialization
  of `Session` (written in `SessionDelta`,
  `meerkat-runtime/src/store/mod.rs:38-41`,
  `sqlite.rs:87-101,224-263`). Same migration surface as §1.1.
- `runtime_states.runtime_state_json` = `RuntimeState`
  (`meerkat-runtime/src/runtime_state.rs:27`). Enum of lifecycle phases,
  no wave-b-typed fields.
- `runtime_boundary_receipts.receipt_json` = `RunBoundaryReceipt`
  (conversation_digest + counts). No wave-b-typed fields.
- `runtime_ops_lifecycle.state_json` = `PersistedOpsSnapshot`
  (optional; `store/mod.rs:177`). No wave-b-typed fields.

### 1.3 `.rkat/sessions/{id}/events.jsonl` — projector output
File: `meerkat-session/src/projector.rs:67-121`. Each line is
`StoredEvent` (`meerkat-session/src/event_store.rs:12-25`) which already
has `schema_version: u32` (`EVENT_SCHEMA_VERSION = 1`). Payload is
`AgentEvent`. AgentEvent variants do not embed `RuntimeTurnMetadata` or
`WireInputState`, but some (`RunStarted`, hot-swap notifications)
carry `ConnectionRef` indirectly when it is present in the session
snapshot echoed back in events. `.rkat/` is derived and disposable
(`CLAUDE.md` rule); replaying the event store regenerates it.

### 1.4 Wire-only shapes (cross-process, not persisted)
`WireInputState` (`meerkat-contracts/src/wire/runtime.rs:235-260`) and
`WireRuntimeTurnMetadata` (same file, commit `18e12d3a4`) are wire
projections only — commit `5fb027af1` explicitly defers the
runtime-side typed projection to wave-c. Not on the persistence
path; mixed-deployment concerns are captured under risk 3.

## 2. Versioning strategy

**Pick: per-entity schema-version byte, piggy-backed on the existing
`Session.version` field plus a new `SESSION_METADATA_SCHEMA_VERSION`
discriminator embedded in the `SessionMetadata` JSON, and a new
`stored_input_state_version: u32` field added to `InputStateSerde`.**

Rationale:
- A single monolithic session version is insufficient. `Session`
  already has one (`session.rs:26`), but `StoredInputState` rows and
  `runtime_states` rows are written by a different store and evolve
  independently.
- A separate version column per table would require a DDL migration
  plus CI plumbing; the existing JSON bodies already have natural
  extension points.
- An opportunistic "try v1, fall back to v0" scheme sounds appealing
  but is footgun-laced: serde is happy to silently accept a v0
  `provider_params: Value::Object({...})` into a v1
  `Option<ProviderParamsOverride>` if any v0 key happens to match a v1
  field (e.g. `temperature`), producing a lossy success. A typed
  discriminator forces an explicit branch.
- The wave-d SDK-regen pass does not care about the Rust envelope; it
  consumes `meerkat-contracts` wire types + JSON schemas, which are
  already versioned via `ContractVersion::CURRENT`. A per-entity
  version inside the Rust persistence layer is orthogonal and does not
  require SDK re-emit.

Concretely:
- Bump `SESSION_VERSION: u32 = 2`.
- Add `#[serde(default)] pub schema_version: u32` to `SessionMetadata`
  (default 1 on read, write 2 after wave-c).
- Add `#[serde(default)] stored_input_state_version: u32` to
  `InputStateSerde` (default 1 on read, write 2).
- Leave `StoredEvent.schema_version` as-is; AgentEvent payloads were
  not retyped.

## 3. v0 → v1 migration matrix

Read-path migrations live in a new
`meerkat-session::persistent::migrations` submodule. Each entry
point reads a `serde_json::Value`, inspects the version
discriminator, dispatches.

### 3.1 `provider_params`: `Value` → `ProviderParamsOverride`
v0 is any `Value::Object` keyed by `temperature`, `top_p`,
`max_output_tokens`, `reasoning`, `thinking_budget_tokens`, plus
arbitrary provider-specific keys. v1 target
(`run_primitive.rs:218-231`) has six typed fields + `provider_tag:
Option<ProviderTag>`.

Mapping: pull known keys by name (number-coerce, lowercase-string →
`ReasoningMode`). Unknown keys move into `ProviderTag` variant
matching the session's `Provider`. If provider is unresolvable,
unknown keys go to `ProviderTag::Unknown { bag:
StructuredProviderExtension }` — which **currently exists only on
wire**; wave-c must add it on core or accept silent drops. Preferred:
add on core. Non-object v0 values (bare numeric/string) → `None` +
retain original under metadata flag `legacy_provider_params_v0`. No
session fails to map outright.

### 3.2 `ConnectionRef`
v0 inner `{realm_id, binding_id, profile}` strings → v1
`{realm: RealmId, binding: BindingId, profile: Option<ProfileId>}`,
each validated via slug regex (`connection.rs:86-90`; rejects empty,
space, `:`). On parse failure, slugify
(lowercase, `[^a-z0-9_.-]` → `_`), retain original under
`legacy_connection_ref`, mark session for operator review. A
silent-drop here causes cross-realm credential bleed
(`session.rs:908-916`). Worst case: after slugification the newtype
still refuses → `connection_ref = None`, session re-resolves against
env defaults on resume (pre-existing behavior for pre-realm sessions).

### 3.3 `RuntimeTurnMetadata` (inside `StoredInputState.persisted_input`)
- `provider_params`: same rule as §3.1.
- `model: Option<String>` → `Option<ModelId>` via `ModelId::new`; on
  slug failure, drop to `None` + trace (the turn was already admitted
  on v0; the override only mattered for a *future* retry).
- `provider: Option<String>` → `Option<Provider>` via
  `Provider::parse_strict`. Unknown → `None` + trace.
- `additional_instructions: Option<Vec<String>>` →
  `Option<Vec<TurnInstruction>>`: lift each `String` to a default
  `TurnInstruction` kind (wave-c locks the exact kind against the v0
  shell semantics of a bare string).
- NEW `connection_ref`, `keep_alive`: absent in v0; default `None`
  via `#[serde(default)]`. No migration work.

### 3.4 Unknown / extra fields
Neither `InputStateSerde` nor `SessionMetadata` sets
`deny_unknown_fields`. Extras drop silently. Wave-c will NOT flip
this — the migration envelope handles known-delta fields; serde's
lenient default covers the rest.

### 3.5 Irrecoverable sessions
None are outright discarded. A v0 `provider` string that no longer
maps just loads as `provider=None`; the session-factory resume path
surfaces the typed "provider missing" error naturally.

## 4. Write-side strategy

**Pick: opportunistic upgrade on read — migrate-in-memory on load,
rewrite as v1 on next successful `save()` / `atomic_apply()`.**

Rationale:
- Single write path (`sqlite_store.rs:71-108`,
  `runtime/store/sqlite.rs:127-150`) already upserts on every session
  save, so the v0 row naturally flips to v1 the first time the session
  does anything. No background sweep required.
- A "migrate lazily forever" stance pollutes the read path with a v0
  fallback that must stay correct across future waves; eventually
  wave-e or wave-f wants to assume v1-only to simplify review.
- A "migrate-all-on-boot" stance forces a maintenance window and blocks
  `rkat` startup on a potentially-large table scan.

Consequence: after wave-c ships, every session that has run at least
one turn is v1. A session that was archived pre-wave-b and never
re-opened stays v0 in the table — that is fine; it migrates the next
time it is loaded, or never, if the operator deletes it.

A single diagnostic CLI (`rkat debug migrate-sessions`, wave-c stretch)
loads + saves every row in one pass for operators who want a clean
snapshot.

## 5. Fixture matrix

Location: `meerkat-session/tests/fixtures/pre_wave_b/`. Each fixture
is a literal JSON byte file (not a helper) so helper drift does not
mask regressions. Loaded by a new
`meerkat-session/tests/persistence_compat.rs`.

Session-blob fixtures:
1. `session_empty_metadata.json` — no `session_metadata` key. Typed
   migration must be a no-op; guards against over-eager rewrite.
2. `session_provider_params_openai.json` — `provider_params: {temperature: 0.2,
   reasoning: "silent", encrypted_content: "..."}`. Expect
   `temperature=Some(0.2)`, `reasoning=Some(Silent)`,
   `provider_tag=Some(ProviderTag::OpenAi{encrypted_content,...})`.
3. `session_provider_params_anthropic_signature.json` — Anthropic
   `signature` untyped key → `ProviderTag::Anthropic{signature}`.
4. `session_provider_params_anthropic_thinking.json` — production
   shape `{thinking: {type:"enabled", budget_tokens:32000}}`. The
   variant most likely to be silently dropped; must land under
   `provider_tag.extension` without loss (see risk 4).
5. `session_provider_params_unknown.json` — non-object value (numeric).
   Expect `None` + legacy value retained under `legacy_provider_params_v0`.
6. `session_connection_ref_slug_valid.json` — `{realm_id:"dev",
   binding_id:"default_openai", profile:null}`; clean typed parse.
7. `session_connection_ref_slug_invalid.json` — `realm_id:"dev mode"`;
   slugified to `dev_mode` + legacy retention.
8. `session_hot_swap_identity_mixed.json` — `SessionLlmIdentity`
   v0 shape but `SessionMetadata` v1 shape (inconsistent mid-flash).
   Both independently migrated; no cross-contamination.

Runtime-store fixtures:
9. `input_state_prompt_full_turn_metadata.json` — `StoredInputState`
   whose `persisted_input.Prompt.turn_metadata` has v0 `provider_params`,
   v0 `additional_instructions: ["foo","bar"]`, v0 `model: "gpt-4o-mini"`.
   Expect typed `TurnInstruction` list + best-effort `ModelId`.
10. `input_state_continuation_minimal.json` — no turn metadata at all;
    verifies `serde(default)` for the NEW `connection_ref`/`keep_alive`
    fields on pre-B6 rows.
11. `input_state_provider_unknown_string.json` — retired provider
    string; `provider=None`, session still loadable.
12. `runtime_session_snapshot_drift.json` — a
    `runtime_session_snapshots` row that drifts from the `sessions`
    row (crash-recovery scenario, `persistent.rs:406-417`). Migration
    picks the newer `updated_at` and migrates the winner only.

## 6. Crates touched

| Crate | Migration-adjacent paths | Wave-b ref |
|---|---|---|
| `meerkat-core` | `session.rs` SessionMetadata/SessionLlmIdentity serde; `connection.rs` RealmId/BindingId/ProfileId newtypes; `lifecycle/run_primitive.rs` RuntimeTurnMetadata V3 + ProviderParamsOverride | `197a70b4a`, `0cd3e4430`, `18e12d3a4`, `cf90208b7` |
| `meerkat-contracts` | `wire/runtime.rs` WireInputState typed fields + StructuredProviderExtension; `wire/connection.rs` WireConnectionRef | `5fb027af1`, `974e81799`, `7eeaf66ac` |
| `meerkat-runtime` | `input_state.rs` InputStateSerde bump + default; `input.rs` turn_metadata field; `store/sqlite.rs` read path | (wave-c only) |
| `meerkat-store` | `sqlite_store.rs` session_json read path — delegates to core Session serde | (wave-c only) |
| `meerkat-session` | `persistent.rs` (new `migrations` submodule); `tests/persistence_compat.rs` (new) | (wave-c only) |

Wave-d SDK regen (`make regen-schemas`) is triggered by the
`meerkat-contracts` movement of `ConnectionRef` to typed newtypes,
not by the persistence-migration work itself. Python/TypeScript SDKs
re-emit from the wire types and do not see the persistence envelope.

## 7. Risk register

1. **v0 row in prod that can't be migrated.** Symptom: `load()`
   returns `SessionError::Agent(InternalError)` from the serde bail at
   `persistent.rs:395` / `739`. Mitigation: wrap translation in
   `fn migrate(Value) -> Result<Session, SessionMigrationError>` and
   emit `SessionMigrationError::Partial` (legacy payload retained)
   rather than failing load. Catching assertion: a compat test that
   loads every fixture and asserts `Ok(Some(_))`; plus fixture 11
   (unknown provider string).

2. **Roll-back after v1 writes.** `RealmId`/`BindingId` newtypes are
   structural, so v0 binaries that expect `{realm_id, binding_id}`
   choke on v1 `{realm, binding}`. Catching assertion: `cargo test -p
   meerkat-store --test rollback_v1_to_v0` — serialises v1 Session,
   attempts v0 deserialise, expects loud failure (not silent
   corruption). Operator policy: no rollback across wave-c without
   `rkat debug export-sessions` first.

3. **Mixed v0/v1 readers against the same DB.** Two
   `SqliteSessionStore` handles (CLI + RPC daemon), one pre- one
   post-wave-c. v1 writer upgrades on save; v0 reader then hits risk
   2. Mitigation: stamp the DB with `PRAGMA user_version = 2`; old
   binaries refuse to open and log "DB newer than binary" rather than
   hitting a parse error deep inside serde. Lockstep upgrade in
   release notes.

4. **Fixture miss most likely to bite.** Real-world
   `meerkat-mob` member with Anthropic extended-thinking overrides:
   `{thinking: {type:"enabled", budget_tokens:32000}}`, not the flat
   `thinking_budget_tokens: u32` v1 offers. Fixture 4 above covers
   it; belt-and-braces: a mob-spawn integration test
   `test_v0_mob_member_with_anthropic_thinking` replaying a real
   payload captured from `meerkat-mob` tests.

5. **`realm_id` vs `connection_ref.realm` drift.**
   `SessionMetadata.realm_id: Option<String>` (session.rs:871) and
   `SessionMetadata.connection_ref.realm: RealmId` are redundant
   post-wave-b. A v0 session with `realm_id="dev"` and no
   `connection_ref` must derive `connection_ref.realm` from it; else
   every resume lands on env defaults. Catching assertion: compat
   test asserting post-load `connection_ref.realm.as_str() ==
   realm_id.unwrap()` when both exist; post-load
   `connection_ref = Some(_)` when only `realm_id` was set on v0.
   Wave-c+1 cleanup: delete the redundant top-level `realm_id`.

---

Summary: persistence-layer migration is localized to two JSON blob
surfaces (`sessions.session_json` and `runtime_input_states.state_json`),
both already carry version bytes (or can be given one via a
`#[serde(default)]` field), and the retype damage is mechanical in
every dimension except `provider_params`, which requires a provider-aware
unknown-keys promotion path. Opportunistic rewrite on next save keeps
the code path single-file and avoids a boot-time migration sweep.
