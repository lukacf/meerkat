# E: Runtime concepts, governance, module and unified runtime guides

[Audit index](README.md) | [Coverage](coverage.md)

Original documentation and initial evidence line ranges refer to baseline `af82b6b3ab34faed9bf3e962d148d55f10dcd1dc`, unless an external dependency or historical revision is explicitly identified. Final-review citations refer to the corrected files in this change. Source excerpts may be de-indented or omit intervening lines; cited ranges identify the complete context. Quoted defects are preserved as evidence, not current usage guidance.

## E-001: Roster guide still describes the fixed empty-context callback defect as current behavior

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/roster.mdx:42-52`**

```text
The roster callback does not hand you the definition. On `rpc_gateway` the
gateway sends `callback/roster_provider/roster` with the serialized
`RosterContext` (`mob_definition`, always the definition the gateway booted
with, and `previous_identities`) as the request params, but the Python and
TypeScript SDK dispatchers pass the provider `params["context"]`, a key the
gateway does not send.
```

Hosts are told they cannot consume the supplied definition and must maintain a duplicate parsed configuration. The guide contradicts itself and teaches incorrect dict access for the current Python context type.

**`meerkat-mobkit/src/identity_first/gateway_bridges.rs:391-402`**

```text
let params = json!({ "context": context });
```

The production bridge now explicitly nests the serialized RosterContext under context, which is exactly the SDK envelope the paragraph says is missing.

**`sdk/python/meerkat_mobkit/agent_builder.py:749-759`**

```text
context = RosterContext.from_dict(params.get("context") or {})
            specs = await provider.roster(context)
```

Python passes a typed RosterContext, not an empty dict. Its mob_definition attribute holds the compiled definition.

**`sdk/typescript/src/agent-builder.ts:517-525`**

```text
const context = parseRosterContext(params.context ?? {});
      const specs = await this._rosterProvider.roster(context);
```

TypeScript reads the same now-present envelope and supplies its typed camelCase context.

**`meerkat-mobkit/src/identity_first/gateway_bridges.rs:1241-1285`**

```text
context["mob_definition"]["id"],
            json!("roster-context-envelope")
```

The explicit regression test checks that the boot definition survives under params.context. The document's later Roster provider context section already describes the fixed contract correctly.

### Independent adjudication

The opening roster section presents an obsolete bug as current behavior, not as a historical warning. I followed both ends of the callback rather than relying on either paragraph: Rust nests the context, Python constructs a RosterContext, and TypeScript constructs the camelCase equivalent. Tests independently pin both fields and the compiled definition shape. Empty contexts remain a backwards-compatibility fallback, but that does not justify describing the bundled gateway as sending one. The optional host-owned TOML policy example is valid and need not be deleted.

**`meerkat-mobkit/src/identity_first/gateway_bridges.rs:391-401`**

```text
let params = json!({ "context": context });
```

The actual GatewayRosterProvider request supplies the envelope the disputed text says is absent.

**`sdk/python/meerkat_mobkit/agent_builder.py:753-758`**

```text
context = RosterContext.from_dict(params.get("context") or {})
```

The Python callback receives a typed object, not the raw dict described in the stale paragraph.

**`sdk/python/tests/test_identity_first_builder_dispatcher.py:553-560`**

```text
assert context.mob_definition["id"] == "team"
```

The regression asserts the compiled definition is accessible through the typed context. It also checks previous_identities.

**`sdk/typescript/tests/identity-first.test.ts:1788-1796`**

```text
previousIdentities: ["a:main", "b:main"],
```

The independent TypeScript callback regression expects mobDefinition and previousIdentities, corroborating the dispatcher at agent-builder.ts:517-525.

**Required correction:** Replace the empty-context defect/workaround claim with the params.context contract and link to Roster provider context. Access context.mob_definition in Python and context.mobDefinition in TypeScript. Preserve the host-owned TOML roster derivation only as an optional policy choice; distinguish TOML definition["mob"] from the compiled context definition's top-level id/orchestrator/profiles fields.

### Changes and final verification

**Changed:** `docs/concepts/roster.mdx`.

Replaced the obsolete empty-context callback defect with the actual params.context envelope and typed Python/TypeScript attribute access. Distinguished the compiled definition's top-level fields from source TOML [mob] nesting. Retained host-owned TOML roster derivation as an optional policy choice and linked the authoritative context section, including K-005's call-site qualification.

**Validation:** Source-checked gateway_bridges.rs:391-401, both SDK dispatchers and their context models; adjudicated callback regression evidence remains exact. Corrected-text assertions and all three Python snippet AST checks passed.

**Final review: pass.** The obsolete empty-context defect is removed. The guide correctly distinguishes params.context, typed Python/TypeScript access, compiled top-level definition fields, and optional host-owned TOML policy. The prior-identity qualification is present both immediately and in the linked field contract. Independent execution of the production Python dispatcher confirmed the nested context and both empty/absent-context compatibility fallbacks.

**`docs/concepts/roster.mdx:42-49`**

```text
`RosterContext` under `params.context`. Both SDKs pass a typed context to
`roster(context)`: read `context.mob_definition` in Python or
`context.mobDefinition` in TypeScript for the booted mob's compiled definition.
```

The corrected API access matches both dispatchers rather than treating the context as an empty raw dictionary.

**`docs/concepts/roster.mdx:51-54`**

```text
A host may instead derive a profile-shaped crew from its own parsed `mob.toml`
as an explicit policy choice, not as a workaround for a missing callback
definition.
```

The useful TOML example is preserved without retaining its false workaround justification.

**`meerkat-mobkit/src/identity_first/gateway_bridges.rs:391-401`**

```text
let params = json!({ "context": context });
```

The shipping bridge supplies the nested envelope.

**`sdk/python/meerkat_mobkit/agent_builder.py:753-758`**

```text
context = RosterContext.from_dict(params.get("context") or {})
```

The actual Python dispatcher constructs the documented typed context; the direct regression exercised this production path.

**`sdk/typescript/src/types.ts:2663-2675`**

```text
mobDefinition: definition as Record<string, unknown>,
    previousIdentities: asStringArray(d.previous_identities),
```

TypeScript lowers the Rust wire fields to the documented camelCase interface.

## E-002: JSON session adapter is described as per-session files with a nonexistent base_dir constructor

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/sessions.mdx:22-30`**

```text
The default adapter writes session state snapshots as JSON files to a local directory. Each session gets its own file, and concurrent access is managed through lock files.
```

A reader chooses incorrect backup/locking granularity and copies a constructor that cannot compile.

**`meerkat-mobkit/src/runtime/session_store.rs:53-58`**

```text
pub struct JsonFileSessionStore {
    data_path: PathBuf,
    lock_path: PathBuf,
    stale_lock_threshold: Duration,
}
```

There is no base_dir field, and the fields are private, so the adjacent Rust struct literal is not a supported constructor.

**`meerkat-mobkit/src/runtime/session_store.rs:153-161`**

```text
pub fn new(data_path: impl AsRef<Path>) -> Self {
        let data_path = data_path.as_ref().to_path_buf();
        let lock_path = data_path.with_extension("lock");
```

The caller supplies one data-file path; the default lock belongs to that file rather than to a session ID.

**`meerkat-mobkit/src/runtime/session_store.rs:190-210`**

```text
let mut persisted = self.read_rows()?;
        persisted.extend(rows.iter().cloned());
```

append_rows reads the aggregate row vector, extends it, and rewrites the single data path. Rows from multiple sessions share that file and lock.

**`meerkat-mobkit/src/unified_runtime/builder.rs:1916-1939`**

```text
"SqliteSessionStore"
```

The definition-based persistent builder defaults to SQLite, not this operational JSON adapter; the page's unqualified 'default adapter' is also misleading.

### Independent adjudication

The page's explicit operational-adapter caveat does not rescue an inaccessible struct literal or the claimed per-session file layout. The adapter has one data path, derives one lock path from it, and rewrites an aggregate JSON array. Its public constructor accepts a file path. I also checked whether 'default' could describe the persistent canonical builder: that path selects SqliteSessionStore unless overridden. Keep the correction scoped to this optional adapter rather than replacing the page with canonical-store documentation.

**`meerkat-mobkit/src/runtime/session_store.rs:53-58`**

```text
data_path: PathBuf,
```

The three fields are private and include data_path, lock_path and stale_lock_threshold, not base_dir.

**`meerkat-mobkit/src/runtime/session_store.rs:153-164`**

```text
let lock_path = data_path.with_extension("lock");
```

Lock naming is file-scoped, not derived from session_id.

**`meerkat-mobkit/src/runtime/session_store.rs:190-210`**

```text
persisted.extend(rows.iter().cloned());
```

append_rows extends the previously read vector and serializes it back to the single configured file; read_rows deserializes Vec<SessionPersistenceRow>.

**`meerkat-mobkit/src/unified_runtime/builder.rs:1916-1938`**

```text
"SqliteSessionStore"
```

The persistent definition-based builder's default session store is SQLite, not the operational JSON adapter.

**Required correction:** Call JsonFileSessionStore an optional operational adapter storing an array of session rows in one caller-selected JSON file. Replace the literal with JsonFileSessionStore::new("/var/mobkit/sessions.json"). Correct lock naming to data_path.with_extension("lock") (sessions.lock in the example), mention with_lock_path, and remove the unqualified default-store assertion.

### Changes and final verification

**Changed:** `docs/concepts/sessions.mdx`.

Described JsonFileSessionStore as an optional operational adapter storing one aggregate JSON array, not a default canonical or per-session-file store. Replaced the inaccessible literal with the public new(data_path) constructor and documented the shared sessions.lock path and with_lock_path override.

**Validation:** Checked runtime/session_store.rs:53-58,153-164,182-224 and the crate-root exports in lib.rs. Constructor, aggregate-array write path, and data_path.with_extension("lock") match the corrected example and prose.

**Final review: pass.** The JSON adapter is now explicitly optional and operational, with one aggregate row-array file and a shared write lock. The public constructor, sessions.lock derivation, and override are correct. The existing separation from Meerkat canonical durability is preserved; persistent builder defaults remain SQLite.

**`docs/concepts/sessions.mdx:22-30`**

```text
`JsonFileSessionStore` is an optional operational adapter. It stores a JSON
array of `SessionPersistenceRow` records in one caller-selected file
```

Removes both the default-store and per-session-file claims.

**`docs/concepts/sessions.mdx:30-37`**

```text
let store = JsonFileSessionStore::new("/var/mobkit/sessions.json");
```

The replacement uses an exported public constructor instead of an inaccessible struct literal.

**`meerkat-mobkit/src/runtime/session_store.rs:153-164`**

```text
let lock_path = data_path.with_extension("lock");
```

The documented default sessions.lock path and with_lock_path override match the implementation.

**`meerkat-mobkit/src/runtime/session_store.rs:190-210`**

```text
persisted.extend(rows.iter().cloned());
```

append_rows extends and rewrites the aggregate array; read_rows deserializes Vec<SessionPersistenceRow>.

## E-003: SessionPersistenceRow field table names nonexistent fields and omits payload, tombstones and labels

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/sessions.mdx:59-63`**

```text
| `session_id` | `String` | Unique session identifier |
| `turn_number` | `u64` | Current turn count |
| `state` | `Value` | Serialized session state (messages, tool results, context) |
| `created_at_ms` | `u64` | Creation timestamp |
| `updated_at_ms` | `u64` | Last update timestamp |
```

Generated row producers/readers target the wrong schema. In JSON inputs, an undocumented state key is not the payload field, and omission of deleted loses the documented backend's deletion semantics.

**`meerkat-mobkit/src/runtime/session_store.rs:24-36`**

```text
pub struct SessionPersistenceRow {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub updated_at_ms: u64,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}
```

This is the complete authoritative serialized shape. turn_number, state and created_at_ms are not fields; payload, deleted and labels are.

**`meerkat-mobkit/tests/session_store_jsonl.rs:151-180`**

```text
deleted: true,
            payload: json!({}),
```

The adapter test uses an explicit deletion row; tombstones are a real contract, not an optional documentation refinement.

### Independent adjudication

The table purports to list top-level SessionPersistenceRow fields, so turn_number/state/created_at_ms cannot be defended as examples of arbitrary payload contents. The full public struct has session_id, updated_at_ms, deleted, payload and labels. Concrete JSON and BigQuery tests construct that shape and explicitly use tombstones. This is a schema correction, not a request for extra detail.

**`meerkat-mobkit/src/runtime/session_store.rs:24-36`**

```text
pub labels: BTreeMap<String, String>,
```

The complete struct defines the five real fields and no turn_number, state or created_at_ms.

**`meerkat-mobkit/tests/session_store_jsonl.rs:151-180`**

```text
deleted: true,
```

The backend test writes a tombstone for s1 and later updates for s2 using payload, demonstrating the omitted fields' concrete semantics.

**Required correction:** Replace the table with session_id: String; updated_at_ms: u64; deleted: bool; payload: serde_json::Value; labels: BTreeMap<String, String>. Describe payload as application JSON and deleted as a tombstone flag. Remove the three nonexistent top-level fields.

### Changes and final verification

**Changed:** `docs/concepts/sessions.mdx`.

Replaced the invented row fields with session_id, updated_at_ms, deleted, payload, and labels, including their actual types, tombstone semantics, and Serde defaults. Clarified that application state belongs in payload.

**Validation:** Automated comparison of the corrected table against public SessionPersistenceRow fields in runtime/session_store.rs:24-36 passed with all five fields and no extras; types and defaults were checked directly.

**Final review: pass.** The five documented fields match the public struct exactly, including their types, tombstone meaning, arbitrary JSON payload, and string-map labels. All five really have serde(default). The nonexistent top-level fields are removed, and application turn/state data is correctly directed into payload.

**`docs/concepts/sessions.mdx:88-95`**

```text
| `deleted` | `bool` | Tombstone flag; a latest row with `true` hides the session from live-row reads |
| `payload` | `serde_json::Value` | Application JSON stored with the row |
| `labels` | `BTreeMap<String, String>` | String key-value metadata |
```

The restored fields replace the invented turn_number/state/created_at_ms contract.

**`meerkat-mobkit/src/runtime/session_store.rs:24-36`**

```text
pub struct SessionPersistenceRow {
```

Independent field extraction compared all five fields against the table in order and passed; each source field has serde(default).

## E-004: SessionStoreContract is a capability-description struct, not the documented CRUD implementation interface

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/sessions.mdx:67-76`**

```text
The `SessionStoreContract` defines the operations every backend must implement:
```

An embedder implementing or calling the alleged public SessionStoreContract operations cannot compile and is misdirected away from the real adapter API.

**`meerkat-mobkit/src/runtime/session_store.rs:12-22`**

```text
pub struct SessionStoreContract {
    pub store: SessionStoreKind,
    pub latest_row_per_session: bool,
    pub tombstones_supported: bool,
    pub dedup_read_path: bool,
    pub file_locking: bool,
    pub crash_recovery: bool,
    pub bigquery_dataset: Option<String>,
    pub bigquery_table: Option<String>,
}
```

The type is data describing the two operational backends; it declares no write/read/list/delete methods and cannot be implemented as a trait.

**`meerkat-mobkit/src/runtime/session_store.rs:182-224`**

```text
pub fn append_rows(
```

The JSON adapter exposes append_rows and row-materialization reads rather than the asserted per-session CRUD API.

**`meerkat-mobkit/src/runtime/session_store.rs:371-447`**

```text
pub async fn stream_insert_rows(
```

BigQuery has a distinct asynchronous insertion interface. The actual tests exercise each concrete adapter, not implementations of the alleged shared CRUD trait.

### Independent adjudication

I checked both concrete implementations and the exported contract constructor. SessionStoreContract is serializable capability data, not a trait, and neither backend has the claimed per-session write/read/list/delete interface. The adapters do share materialization semantics and have integration coverage, so do not erase that legitimate relationship; remove only the invented implementation interface and implication that a common CRUD trait is being tested.

**`meerkat-mobkit/src/runtime/session_store.rs:12-22`**

```text
pub struct SessionStoreContract {
```

The type consists of a store discriminator, capability flags, and optional BigQuery dataset/table values.

**`meerkat-mobkit/src/runtime/session_store.rs:105-128`**

```text
pub fn session_store_contracts(decisions: &RuntimeDecisionState) -> Vec<SessionStoreContract> {
```

This factory returns two descriptions; it does not construct implementations of an interface.

**`meerkat-mobkit/src/runtime/session_store.rs:182-224`**

```text
pub fn read_live_rows(&self) -> Result<Vec<SessionPersistenceRow>, JsonFileSessionStoreError> {
```

JSON's concrete API is append_rows plus row-array reads, not the documented CRUD operations.

**`meerkat-mobkit/tests/session_store_jsonl.rs:199-215`**

```text
.stream_insert_rows(&writes)
```

The BigQuery test calls its distinct asynchronous insertion API, followed by read_latest_rows/read_live_rows.

**Required correction:** Describe SessionStoreContract as capability metadata returned by session_store_contracts(decisions), including its flags and optional BigQuery names. Document JsonFileSessionStore::append_rows and BigQuerySessionStoreAdapter::stream_insert_rows plus each adapter's read_rows/read_latest_rows/read_live_rows methods. Remove the nonexistent shared CRUD interface and common-trait conformance assertion; adapter tests may still be mentioned accurately.

### Changes and final verification

**Changed:** `docs/concepts/sessions.mdx`.

Replaced the nonexistent CRUD trait contract with SessionStoreContract capability metadata, all capability fields and their current backend values. Documented the concrete synchronous JSON and asynchronous BigQuery insertion/read APIs and retained an accurately scoped statement about adapter integration coverage.

**Validation:** Checked session_store_contracts at runtime/session_store.rs:105-128, JSON APIs at 182-224, BigQuery APIs at 371-465, and the cited concrete-adapter integration test evidence. Corrected-text assertions passed.

**Final review: pass.** SessionStoreContract is accurately documented as capability metadata, not a CRUD trait. All flags and optional BigQuery names match the factory, and JSON synchronous append/read APIs are separated from BigQuery asynchronous insert/read APIs. The integration-coverage statement no longer invents common-trait conformance.

**`docs/concepts/sessions.mdx:99-101`**

```text
`SessionStoreContract` is serializable capability metadata returned by
`session_store_contracts(&decisions)`, not an implementation trait or shared
CRUD interface:
```

Corrects the type's role rather than retaining imaginary methods.

**`docs/concepts/sessions.mdx:120-126`**

```text
| `JsonFileSessionStore` | Synchronous `append_rows(&rows)` | Synchronous `read_rows()`, `read_latest_rows()`, `read_live_rows()` |
| `BigQuerySessionStoreAdapter` | Asynchronous `stream_insert_rows(&rows).await` | Asynchronous `read_rows().await`, `read_latest_rows().await`, `read_live_rows().await` |
```

The actual concrete methods and async distinctions are preserved.

**`meerkat-mobkit/src/runtime/session_store.rs:105-128`**

```text
pub fn session_store_contracts(decisions: &RuntimeDecisionState) -> Vec<SessionStoreContract> {
```

The factory returns metadata for both stores; latest/dedup/tombstones are true for both, while file locking and crash recovery are JSON-only.

**`meerkat-mobkit/src/runtime/session_store.rs:371-374`**

```text
pub async fn stream_insert_rows(
```

The BigQuery API is a concrete async insertion method, not a shared write operation.

## E-005: Live session-row materialization means non-tombstoned, not currently running

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/sessions.mdx:82-83`**

```text
- **`materialize_live_session_rows`** -- returns rows for currently active sessions
```

Operational code can miscount idle, stopped or otherwise inactive persisted sessions as active execution and make incorrect capacity/health decisions.

**`meerkat-mobkit/src/runtime/session_store.rs:146-151`**

```text
pub fn materialize_live_session_rows(rows: &[SessionPersistenceRow]) -> Vec<SessionPersistenceRow> {
    materialize_latest_session_rows(rows)
        .into_iter()
        .filter(|row| !row.deleted)
        .collect()
}
```

The helper only groups rows and removes the latest tombstones. It does not query runtime liveness, member status or a session service.

**`meerkat-mobkit/src/runtime/session_store.rs:130-143`**

```text
Some(existing) => row.updated_at_ms >= existing.updated_at_ms,
```

Latest is based on updated_at_ms; there is no active/running predicate anywhere in materialization.

### Independent adjudication

Although 'live' is sometimes used informally for a nondeleted record, the document explicitly says 'currently active sessions', which implies runtime liveness. The helper only selects the greatest updated_at_ms per ID and filters deleted. BigQuery's corresponding SQL uses exactly the same storage-visibility predicate. Neither checks a runtime, actor, turn, or member status.

**`meerkat-mobkit/src/runtime/session_store.rs:130-151`**

```text
.filter(|row| !row.deleted)
```

This is the complete additional criterion applied by materialize_live_session_rows after latest-row selection.

**`meerkat-mobkit/src/runtime/session_store.rs:452-463`**

```text
) WHERE deleted = false
```

The concrete BigQuery read_live_rows also selects non-tombstoned latest records rather than executing a liveness query.

**Required correction:** Define materialize_live_session_rows as returning each session's latest row only when that latest row has deleted == false. State explicitly that this is storage visibility, not evidence that a session is currently active or executing.

### Changes and final verification

**Changed:** `docs/concepts/sessions.mdx`.

Defined latest-row materialization by updated_at_ms, including the helper's input-order tie break, and live rows as latest non-tombstoned records. Explicitly separated storage visibility from active execution.

**Validation:** Checked materialize_latest_session_rows/materialize_live_session_rows at runtime/session_store.rs:130-151 and BigQuery live-row SQL at 452-463. No runtime-liveness predicate exists in these paths.

**Final review: pass.** Live rows now mean latest non-tombstoned storage records, explicitly not executing sessions. The added greatest-timestamp and later-input tie-break explanation is accurate for the named materialization helper; it is not incorrectly extended to BigQuery's SQL tie ordering.

**`docs/concepts/sessions.mdx:132-138`**

```text
- **`materialize_latest_session_rows`** -- returns the row with the greatest
  `updated_at_ms` per session; a later input row wins a timestamp tie
- **`materialize_live_session_rows`** -- returns each session's latest row only
  when that row has `deleted == false`
```

Precisely describes selection and visibility without promising runtime liveness.

**`meerkat-mobkit/src/runtime/session_store.rs:130-151`**

```text
Some(existing) => row.updated_at_ms >= existing.updated_at_ms,
```

The >= comparison makes the later input win ties, followed solely by the !row.deleted filter.

**`meerkat-mobkit/src/runtime/session_store.rs:452-463`**

```text
) WHERE deleted = false
```

The BigQuery live view likewise uses storage visibility, with no actor/session execution probe.

## E-006: JSON stale-lock recovery omits the mandatory age threshold

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/sessions.mdx:32-32`**

```text
**Stale lock recovery.** If a process crashes while holding a lock, the lock becomes stale. On the next access attempt, the store checks whether the PID in the lock record is still alive. Dead PIDs trigger automatic lock recovery -- the stale lock is removed and a new one is acquired.
```

A host following the page expects the first post-crash write to recover automatically, but instead receives LockHeld until the age gate opens.

**`meerkat-mobkit/src/runtime/session_store.rs:154-161`**

```text
stale_lock_threshold: Duration::from_secs(30),
```

The default has an explicit 30-second age guard.

**`meerkat-mobkit/src/runtime/session_store.rs:276-286`**

```text
if age_ms < stale_threshold_ms {
                return Ok(false);
            }
            return Ok(!is_process_alive(record.owner_pid));
```

Even a dead PID's fresh lock is not considered stale, and its PID is not probed until the threshold is reached.

**`meerkat-mobkit/tests/session_store_jsonl.rs:41-54`**

```text
.with_stale_lock_threshold(Duration::from_millis(10));
```

The recovery test deliberately supplies both an aged lock record and a smaller threshold; it does not establish immediate recovery after a crash.

### Independent adjudication

The existing recovery prose leaves no age condition between a crash and the next access. The implementation tests age before probing the owner, so an already-dead owner does not make a fresh valid lock recoverable. The aged-live-owner regression also disproves an age-only interpretation. Malformed locks deliberately use modification age rather than a PID. These are important writer retry semantics, not merely an internal implementation detail.

**`meerkat-mobkit/src/runtime/session_store.rs:153-172`**

```text
stale_lock_threshold: Duration::from_secs(30),
```

The threshold defaults to 30 seconds and has a public with_stale_lock_threshold override.

**`meerkat-mobkit/src/runtime/session_store.rs:276-295`**

```text
return Ok(!is_process_alive(record.owner_pid));
```

This liveness probe is reached only after the preceding age_ms < stale_threshold_ms early return; malformed-record fallback checks file modification age.

**`meerkat-mobkit/tests/session_store_jsonl.rs:111-148`**

```text
.expect_err("aged lock with live owner should block writer");
```

The regression explicitly preserves an old lock whose process is still alive. The dead-owner recovery test uses an aged record, not an immediate crash.

**Required correction:** Explain that a valid lock must reach the configurable stale_lock_threshold (30 seconds by default) and have a dead owner before recovery. Fresh dead-owner locks still produce LockHeld; old live-owner locks remain protected. For malformed records the store uses the lock file's modification age. Scope this behavior to write-lock acquisition.

### Changes and final verification

**Changed:** `docs/concepts/sessions.mdx`.

Scoped stale-lock recovery to write-lock acquisition and documented the configurable 30-second age threshold plus dead-owner requirement, fresh dead-owner LockHeld behavior, protection for aged live owners, and malformed-record modification-age fallback.

**Validation:** Read runtime/session_store.rs:153-172,228-295 and checked the adjudicated live/dead-owner test excerpts. Corrected-text assertions cover the age gate, public setter, and malformed-record fallback.

**Final review: pass.** Recovery is now scoped to write-lock acquisition and requires both age and a dead owner for a valid record. The default and public override are correct, fresh dead-owner locks remain blocking, aged live-owner locks remain protected, and malformed records use modification age. No runtime behavior was changed.

**`docs/concepts/sessions.mdx:39-46`**

```text
is recoverable only after it reaches `stale_lock_threshold` and its owner PID
is no longer alive. The default threshold is 30 seconds
```

The missing age prerequisite and its default are restored.

**`meerkat-mobkit/src/runtime/session_store.rs:153-172`**

```text
stale_lock_threshold: Duration::from_secs(30),
```

The source default and with_stale_lock_threshold setter match the prose.

**`meerkat-mobkit/src/runtime/session_store.rs:276-295`**

```text
if age_ms < stale_threshold_ms {
                return Ok(false);
            }
            return Ok(!is_process_alive(record.owner_pid));
```

The early age gate precedes the PID probe; the malformed-record branch separately compares file modification age.

## E-007: BigQuery adapter example cannot construct the public type and omits required project/auth setup

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/sessions.mdx:36-43`**

```text
BigQuerySessionStoreAdapter {
    dataset: "mobkit_sessions",
    table: "session_rows",
}
```

The documented Rust example fails to compile, and merely replacing the literal with a constructor still fails at runtime unless the host supplies project and token configuration.

**`meerkat-mobkit/src/runtime/session_store.rs:84-93`**

```text
pub struct BigQuerySessionStoreAdapter {
    dataset: String,
    table: String,
    project_id: Option<String>,
    api_base_url: String,
    access_token: Option<String>,
    http_timeout: Duration,
    client: reqwest::Client,
}
```

All fields are private and additional fields are required. The documented literal is neither a complete nor accessible Rust initializer.

**`meerkat-mobkit/src/runtime/session_store.rs:326-341`**

```text
pub fn new_native(dataset: impl Into<String>, table: impl Into<String>) -> Self {
```

The supported construction surface is new_native followed by optional setters.

**`meerkat-mobkit/src/runtime/session_store.rs:554-564`**

```text
"missing BigQuery project_id: call with_project_id(...) or set BIGQUERY_PROJECT_ID"
```

Dataset and table alone are not enough to perform an operation; project configuration is mandatory.

**`meerkat-mobkit/src/runtime/session_store.rs:567-593`**

```text
"missing BigQuery access token: call with_access_token(...) or set BIGQUERY_ACCESS_TOKEN"
```

The adapter requires an explicitly supplied or supported environment access token; the example does not automatically use an ambient ADC client.

### Independent adjudication

The initializer is not pseudocode labeled as such: it is a Rust block with private, incomplete fields and even &str values for String fields. The supported new_native constructor is clear. Project and token are required for real nonempty operations, but explicit setters are not mandatory because supported environment fallbacks exist; an empty insert is a no-op. The correction must retain that distinction and must not imply ADC discovery.

**`meerkat-mobkit/src/runtime/session_store.rs:84-93`**

```text
project_id: Option<String>,
```

The adapter has private dataset/table/project/client/configuration fields, making the documented public literal invalid.

**`meerkat-mobkit/src/runtime/session_store.rs:326-351`**

```text
pub fn new_native(dataset: impl Into<String>, table: impl Into<String>) -> Self {
```

The public constructor and with_project_id/with_access_token builders are the actual construction surface.

**`meerkat-mobkit/src/runtime/session_store.rs:539-593`**

```text
"GOOGLE_OAUTH_ACCESS_TOKEN",
```

Project resolution uses a configured value or BIGQUERY_PROJECT_ID. Token resolution uses a configured value, then BIGQUERY_ACCESS_TOKEN, GOOGLE_OAUTH_ACCESS_TOKEN, GOOGLE_ACCESS_TOKEN; otherwise it returns Configuration.

**`meerkat-mobkit/tests/session_store_jsonl.rs:203-207`**

```text
.with_project_id("phase5-project")
```

The concrete HTTP integration test supplies both project and access token before using the adapter.

**Required correction:** Show BigQuerySessionStoreAdapter::new_native("mobkit_sessions", "session_rows").with_project_id(project_id).with_access_token(access_token), with host-supplied variables. Alternatively document BIGQUERY_PROJECT_ID and the three supported token environment fallbacks, in precedence order. Do not claim automatic application-default credential lookup or that new_native itself runs the separate BigQuery naming validator.

### Changes and final verification

**Changed:** `docs/concepts/sessions.mdx`.

Replaced the private BigQuery struct literal with new_native(...).with_project_id(project_id).with_access_token(access_token), using host-supplied values. Documented project and all three token environment fallbacks in precedence order, empty-insert behavior, and absence of ADC discovery. Clarified that naming validation is a separate policy call, not constructor behavior, and accurately limited its character set to ASCII.

**Validation:** Checked runtime/session_store.rs:326-351,371-383,539-593; decisions.rs:179-197; and crate-root exports. The construction and naming-validation text matches the public API; no BigQuery credentials or live calls were used.

**Final review: pass.** The new BigQuery example uses the exported constructor and real builder setters, with host-supplied project/token inputs made explicit. Exact fallback precedence, empty-insert behavior, and no-ADC caveat match source. Naming validation is accurately separated from new_native, and its ASCII-only character policy is correct.

**`docs/concepts/sessions.mdx:55-59`**

```text
let store = BigQuerySessionStoreAdapter::new_native("mobkit_sessions", "session_rows")
    .with_project_id(project_id)
    .with_access_token(access_token);
```

The old private/incomplete struct literal is replaced with the supported construction path.

**`docs/concepts/sessions.mdx:62-79`**

```text
The separate `validate_bigquery_naming(&BigQueryNaming)` policy validator
checks dataset and table names; `new_native` does not invoke it:
```

The example does not imply constructor validation or implicit credential discovery.

**`meerkat-mobkit/src/runtime/session_store.rs:326-351`**

```text
pub fn new_native(dataset: impl Into<String>, table: impl Into<String>) -> Self {
```

The constructor and both setters accept the values shown; crate-root exports were independently checked.

**`meerkat-mobkit/src/runtime/session_store.rs:539-593`**

```text
"BIGQUERY_ACCESS_TOKEN",
            "GOOGLE_OAUTH_ACCESS_TOKEN",
            "GOOGLE_ACCESS_TOKEN",
```

These follow a nonempty explicit token; project has its own explicit-then-BIGQUERY_PROJECT_ID resolution.

**`meerkat-mobkit/src/decisions.rs:179-197`**

```text
.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
```

The separate naming policy is ASCII-limited as the corrected guide says.

## E-008: Accepted send responses do not always provide a usable session correlation ID

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/sessions.mdx:111-111`**

```text
When MobKit accepts a `mobkit/send_message` request, the response includes the Meerkat `session_id` that accepted the message. Treat that value as the canonical correlation handle for follow-up inspection, debugging, and resume-aware flows.
```

Consumers can treat an empty string as a canonical session handle and issue invalid follow-up history/debug/resume requests after a successful delivery.

**`meerkat-mobkit/src/rpc/mob_methods.rs:526-541`**

```text
// succeeded (`accepted: true`), so `session_id` may be
                    // empty on success; consumers must treat an empty
                    // `session_id` as "unknown", not as a usable reference.
```

The implementation explicitly documents the successful-send race in which an identity session is materialized then retired/rebound before correlation is read.

**`meerkat-mobkit/src/rpc/mob_methods.rs:535-543`**

```text
.and_then(|status| status.session_id)
                        .or_else(|| resolve_time_session_id.clone())
                        .map(|session_id| session_id.to_string())
                        .unwrap_or_default()),
```

Both missing observations produce an empty string, not an error or a guaranteed accepting-session receipt.

**`meerkat-mobkit/src/rpc/mob_methods.rs:565-575`**

```text
"accepted": true,
                            "member_id": member_id,
                            "session_id": session_id
```

The empty result still appears in a successful accepted response.

### Independent adjudication

The JSON success response always contains a session_id key, so absence of the key is not the defect. The identity arm can produce an empty string while still accepting delivery, and its nonempty value is selected from observed bindings rather than an atomic accepting-session receipt. I checked the obvious counter-evidence: the existing valid-ID test exercises send_message_on_mob, not the identity arm's lazy-materialization/re-read race, and is ignored. It cannot establish the page's universal guarantee.

**`meerkat-mobkit/src/rpc/mob_methods.rs:534-543`**

```text
.or_else(|| resolve_time_session_id.clone())
```

After a successful tracked identity send, the handler uses a post-send status read, falls back to the resolve-time binding, then unwrap_or_default produces an empty String if both observations are absent.

**`meerkat-mobkit/src/rpc/mob_methods.rs:567-575`**

```text
"accepted": true,
```

The same Ok(session_id) arm emits accepted true even when session_id is empty; it does not downgrade the successful send to an error.

**`meerkat-mobkit/tests/unified_console.rs:961-993`**

```text
SessionId::parse(&session_id).expect("send_message should return a valid session_id");
```

This ignored test calls the direct mob-member helper. It offers no guarantee for the identity RPC branch or a concurrent binding change.

**Required correction:** Describe session_id as a best-effort correlation value. Use a nonempty returned value for inspection, but do not promise an atomic receipt identifying the exact accepting session across concurrent identity rebinding. A successful identity-plane send may return an empty string when neither binding can be observed; empty means unknown, not a usable ID and not a failed delivery.

### Changes and final verification

**Changed:** `docs/concepts/sessions.mdx`.

Changed the universal accepting-session guarantee to best-effort correlation. Documented post-send binding observation with resolve-time fallback, successful accepted:true responses with an empty session_id, and the lack of atomic accepting-session identity across rebinding.

**Validation:** Read rpc/mob_methods.rs:514-578, including unwrap_or_default and the accepted:true response. Corrected-text assertions passed; no race reproduction or runtime behavior change was attempted.

**Final review: pass.** The documentation retains the response key but correctly weakens its meaning to best-effort correlation. It explicitly covers successful accepted:true with an empty string and avoids promising the exact accepting session across rebinding. Nonempty values remain useful inspection handles.

**`docs/concepts/sessions.mdx:166-174`**

```text
successful send can return `accepted: true` with `session_id: ""`. Empty means
unknown: it is not a usable session ID and does not mean delivery failed.
```

Clients are no longer told to treat every success value as a usable canonical session reference.

**`meerkat-mobkit/src/rpc/mob_methods.rs:534-543`**

```text
.or_else(|| resolve_time_session_id.clone())
                        .map(|session_id| session_id.to_string())
                        .unwrap_or_default()
```

Successful tracked send uses a post-send observation, then resolve-time fallback, then empty String.

**`meerkat-mobkit/src/rpc/mob_methods.rs:567-575`**

```text
"accepted": true,
```

The empty fallback does not turn a successful delivery into an RPC failure.

## E-009: Both governance validation snippets call nonexistent signatures

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/reference/governance.mdx:17-43`**

```text
validate_governance_state("realignment_in_progress")?;
```

Neither advertised validation example compiles, and a reader is not taught the actual file-content contract or the evidence-bearing traceability input.

**`meerkat-mobkit/src/governance.rs:67-76`**

```text
pub fn validate_governance_state(
    file_name: &str,
    content: &str,
) -> Result<(), GovernanceValidationError> {
```

The function needs a file label and document content containing a governance_state: line, not one bare state string.

**`docs/reference/governance.mdx:39-41`**

```text
validate_traceability_statuses(&["TYPED", "WIRED", "VALIDATED"])?;
```

The second snippet supplies a string slice array rather than a traceability document.

**`meerkat-mobkit/src/governance.rs:94-100`**

```text
pub fn validate_traceability_statuses(markdown: &str) -> Result<(), GovernanceValidationError> {
```

The parser accepts a Markdown table or YAML document string. Valid implemented-status rows also require evidence, so a list of status tokens is not a substitute input.

**`meerkat-mobkit/tests/governance_contracts.rs:70-83`**

```text
validate_governance_state("spec", "governance_state: blocked")
```

The integration test exercises the actual two-argument source-document contract.

### Independent adjudication

Both Rust examples call the actual exported function names with incompatible arguments. These are document parsers, not scalar/status-list validators. A two-argument governance-state call with the required line is supported. For traceability, I followed both Markdown and YAML branches; implemented statuses require non-placeholder evidence, so replacing the array with a bare status string would still be wrong.

**`meerkat-mobkit/src/governance.rs:67-98`**

```text
pub fn validate_traceability_statuses(markdown: &str) -> Result<(), GovernanceValidationError> {
```

The first function takes file_name and content, and the second takes document text, not &[&str].

**`meerkat-mobkit/tests/governance_contracts.rs:70-81`**

```text
validate_governance_state("spec", "governance_state: blocked")
```

The test uses the real two-argument parser and verifies InvalidGovernanceState.

**`meerkat-mobkit/tests/governance_contracts.rs:191-207`**

```text
GovernanceValidationError::MissingTraceabilityEvidence { .. }
```

The YAML TYPED-row regression rejects an empty evidence list, confirming the correction must include evidence-bearing input.

**Required correction:** Use validate_governance_state("spec.yaml", "governance_state: realignment_in_progress\n")?. Use validate_traceability_statuses("rows:\n  - status: TYPED\n    evidence: [src/governance.rs]\n")? or a complete equivalent Markdown table. Explain the file-content contract and that TYPED, WIRED, VALIDATED and PROVISIONAL require non-placeholder evidence; this checks presence, not whether the referenced artifact exists or proves the status.

### Changes and final verification

**Changed:** `docs/reference/governance.mdx`.

Replaced both invalid validation calls with imported, correctly shaped document-input examples: a file label plus governance_state text, and a complete YAML traceability row with evidence. Explained evidence-required statuses and the fact that evidence presence is not proof or artifact-existence validation.

**Validation:** Checked governance.rs:67-98,172-195,211-228 and lib.rs exports. Source-evidence and corrected-text checks passed; the example evidence path exists. Rust snippets were source/type-signature reviewed, not compiled.

**Changed:** `docs/sdks/rust.mdx`.

Propagated confirmed governance signatures and document/evidence semantics to the Rust SDK reference.

**Validation:** Compared signatures to governance.rs:67-70,94-99 and the independently corrected governance guide.

**Final review: pass.** The coordinated Rust SDK signatures now match the exported functions exactly. The new explanation correctly distinguishes diagnostic file labels/document text from a scalar state or status array and retains evidence-bearing traceability requirements. Independently followed both the Markdown/YAML parser dispatch and evidence checks, and inspected the linked corrected governance examples. The SDK signature inventory is not represented as a standalone executable Rust program.

**`docs/sdks/rust.mdx:280-288`**

```text
fn validate_governance_state(file_name: &str, content: &str) -> Result<(), GovernanceValidationError>
fn validate_traceability_statuses(markdown: &str) -> Result<(), GovernanceValidationError>
```

Both previously incorrect argument lists now match source.

**`meerkat-mobkit/src/governance.rs:67-76`**

```text
pub fn validate_governance_state(
    file_name: &str,
    content: &str,
) -> Result<(), GovernanceValidationError> {
```

The function searches supplied contents and uses the file name only in diagnostics.

**`meerkat-mobkit/src/governance.rs:94-100`**

```text
pub fn validate_traceability_statuses(markdown: &str) -> Result<(), GovernanceValidationError> {
    if looks_like_markdown_table(markdown) {
```

The implementation receives document text and dispatches between Markdown and YAML.

**`meerkat-mobkit/src/governance.rs:183-193`**

```text
        if status_requires_evidence(&row.status)
            && row.evidence.iter().all(|entry| is_missing_evidence(entry))
```

The referenced traceability contract genuinely checks non-placeholder evidence, not just status tokens.

**Final review: pass.** Both governance examples now use actual document-input signatures and imported exported functions. The traceability example includes non-placeholder evidence and the guide explicitly limits validation to presence rather than truth/existence. The repeated Rust SDK defect is also closed: its signatures and adjacent explanation now agree with source and link to the complete corrected examples.

**`docs/reference/governance.mdx:24-27`**

```text
validate_governance_state("spec.yaml", "governance_state: realignment_in_progress\n")?;
```

The first argument is an error label, and the second is document text, not a scalar state-only call.

**`docs/reference/governance.mdx:51-55`**

```text
let traceability = r#"rows:
  - status: TYPED
    evidence: [meerkat-mobkit/src/governance.rs]
"#;
validate_traceability_statuses(traceability)?;
```

The complete YAML row satisfies the parser's status/evidence input shape; the referenced example path exists.

**`docs/sdks/rust.mdx:280-281`**

```text
fn validate_governance_state(file_name: &str, content: &str) -> Result<(), GovernanceValidationError>
fn validate_traceability_statuses(markdown: &str) -> Result<(), GovernanceValidationError>
```

The coordinator propagated the correction to the repeated SDK signatures.

**`docs/sdks/rust.mdx:284-289`**

```text
The first function receives a diagnostic file label and document contents, not
a state value or a file to read.
```

The adjacent SDK explanation no longer contradicts the actual parser contracts; it also describes evidence-bearing documents and rejects status arrays.

**`meerkat-mobkit/src/governance.rs:67-98`**

```text
pub fn validate_traceability_statuses(markdown: &str) -> Result<(), GovernanceValidationError> {
```

Both final references match the public source signatures; the YAML branch at 172-195 requires non-placeholder evidence for TYPED.

## E-010: Governance error reference lists three nonexistent enum variants

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/reference/governance.mdx:81-84`**

```text
| `InvalidState` | Governance state not recognized |
| `InvalidTraceabilityStatus` | Status not in strict set |
| `BaselineVerificationFailed` | Required Meerkat symbols missing |
| `ContractNotTraced` | Contract lacks traceability annotation |
```

Rust consumers cannot match the documented variants and are not told about real missing-state, empty-table and malformed-row failures.

**`meerkat-mobkit/src/governance.rs:18-26`**

```text
pub enum GovernanceValidationError {
    MissingGovernanceState { file: String },
    InvalidGovernanceState { file: String, found: String },
    NoTraceabilityRows,
    InvalidTraceabilityStatus { line: usize, status: String },
    MissingTraceabilityEvidence { line: usize },
    InvalidTraceabilityRow { line: usize },
}
```

This complete enum has only one of the four names in the table. Baseline failures belong to the separate BaselineVerificationError type.

### Independent adjudication

The section names GovernanceValidationError, so a generic conceptual-error interpretation does not fit its Rust identifier table. Only InvalidTraceabilityStatus exists among the four listed names. The full enum contains six different variants, while baseline verification has its own exported error type. These are current API references, not archived release facts.

**`meerkat-mobkit/src/governance.rs:18-26`**

```text
MissingGovernanceState { file: String },
```

The six variants are MissingGovernanceState, InvalidGovernanceState, NoTraceabilityRows, InvalidTraceabilityStatus, MissingTraceabilityEvidence and InvalidTraceabilityRow.

**`meerkat-mobkit/src/baseline.rs:30-36`**

```text
MissingSymbols(BaselineVerificationReport),
```

BaselineVerificationError, not GovernanceValidationError::BaselineVerificationFailed, owns missing-symbol reports and repository configuration/path errors.

**Required correction:** Replace the governance error table with the six actual variants and field-aware causes. If documenting baseline errors, give BaselineVerificationError its own table (RepoNotConfigured, RepoMissing, RepoUnreadable, MissingSymbols); do not invent a combined wrapper or ContractNotTraced variant.

### Changes and final verification

**Changed:** `docs/reference/governance.mdx`.

Replaced nonexistent governance errors with all six actual variants and field-aware causes. Explained Markdown source-line versus YAML row-index reporting. Added a separate table for the four BaselineVerificationError variants rather than an invented wrapper.

**Validation:** Compared the tables with governance.rs:18-26,100-195 and baseline.rs:30-36,63-93. All variants and position semantics match the implementation.

**Final review: pass.** All six real governance variants and all four separate baseline variants are documented. The invented InvalidState, ContractNotTraced, and BaselineVerificationFailed entries are removed. Field-aware causes and Markdown source-line versus YAML row-index/parse-error positions match the implementation.

**`docs/reference/governance.mdx:110-118`**

```text
| `MissingGovernanceState { file }` | No `governance_state:` line in the named document |
```

The six-row table now uses the actual enum rather than conceptual or nonexistent names.

**`docs/reference/governance.mdx:120-127`**

```text
| `MissingSymbols(report)` | The scan left unsatisfied checklist entries in `report.missing_symbols` |
```

Baseline failures remain in their distinct error type.

**`meerkat-mobkit/src/governance.rs:18-26`**

```text
MissingTraceabilityEvidence { line: usize },
    InvalidTraceabilityRow { line: usize },
```

An automated source-variant coverage check passed for all six governance variants, and source review verified diagnostic positions.

**`meerkat-mobkit/src/baseline.rs:30-36`**

```text
MissingSymbols(BaselineVerificationReport),
```

All four baseline variants were independently compared with the separate table.

## E-011: Governance validation does not invoke the baseline check or establish release completeness

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/reference/governance.mdx:69-75`**

```text
`validate_governance_contracts` runs the full governance validation suite:

1. Governance state validation
2. Traceability status validation
3. Baseline symbol verification
```

A CI owner can rely on governance_check as proof of baseline compatibility or completed release verification even though it passes independently of both.

**`meerkat-mobkit/src/governance.rs:231-242`**

```text
validate_governance_state(".rct/spec.yaml", spec_yaml)?;
    validate_governance_state(".rct/plan.yaml", plan_yaml)?;
    validate_governance_state(".rct/checklist.yaml", checklist_yaml)?;
    validate_traceability_statuses(traceability_markdown)?;
    Ok(())
```

The complete function only checks three supplied governance texts and the supplied traceability document. There is no baseline call, source contract inventory, test execution or release-completeness check.

**`meerkat-mobkit/src/governance.rs:227-229`**

```text
!matches!(status, "MISSING" | "DEFERRED" | "STUBBED")
```

Unimplemented/deferred/stubbed rows do not even require evidence, directly limiting the opening claim that all contracts are traced, documented and verified before release.

**`meerkat-mobkit/src/bin/governance_check.rs:91-100`**

```text
validate_governance_contracts(&spec, &plan, &checklist, &traceability)
```

The standalone binary invokes the same narrow validator, not a stronger combined release/baseline gate.

### Independent adjudication

I checked the complete validator and standalone binary for an indirect baseline call; neither contains one. The parser accepts MISSING rows with no evidence, and no function walks the codebase to establish that every feature is inventoried or runs tests to verify status truth. Governance remains useful as a format/status gate, but the current opening and numbered suite list promise materially stronger release assurance than it supplies.

**`meerkat-mobkit/src/governance.rs:231-243`**

```text
validate_traceability_statuses(traceability_markdown)?;
```

The function consists only of three validate_governance_state calls, this traceability call, and Ok(()); no baseline or release-completeness checks follow.

**`meerkat-mobkit/src/bin/governance_check.rs:91-100`**

```text
validate_governance_contracts(&spec, &plan, &checklist, &traceability)
```

The CLI uses that same narrow validator and reports success without invoking baseline verification.

**`meerkat-mobkit/tests/governance_contracts.rs:179-190`**

```text
validate_traceability_statuses(yaml).expect("yaml traceability should validate");
```

The passing fixture contains status MISSING and evidence: [], directly contradicting any release-completeness guarantee.

**Required correction:** Describe governance validation as checking required governance_state text plus traceability row format, recognized statuses and non-placeholder evidence where required. Remove baseline symbol verification from validate_governance_contracts and narrow claims that all contracts/features are traced and verified before release. State that baseline_check or verify_meerkat_baseline_symbols is a separate optional operation, not a release-compatibility substitute.

### Changes and final verification

**Changed:** `docs/reference/governance.mdx`.

Narrowed governance promises to declaration, row-format, status, and evidence-presence validation. Documented the four supplied texts accepted by validate_governance_contracts, removed the nonexistent baseline phase, and separated optional baseline invocation from release-completeness assurance.

**Validation:** Checked the complete validator at governance.rs:231-243, governance_check.rs:91-100 via adjudicated evidence, and the accepted MISSING-with-empty-evidence fixture. Corrected-text assertions passed.

**Final review: pass.** The opening and contracts section now describe declaration/format/status/evidence-presence checks rather than release completeness. The four supplied strings and three state checks are accurate. Baseline verification is explicitly separate and optional, and no test execution or exhaustive feature inventory is promised.

**`docs/reference/governance.mdx:7-10`**

```text
They do not discover every contract,
verify the truth of status claims, or establish release completeness.
```

The unsupported release assurance is removed at its original top-level occurrence.

**`docs/reference/governance.mdx:93-104`**

```text
Neither invokes baseline verification, executes tests, nor checks that all
features have traceability rows.
```

The suite and binary descriptions no longer imply a hidden baseline/completeness phase.

**`meerkat-mobkit/src/governance.rs:231-243`**

```text
validate_governance_state(".rct/spec.yaml", spec_yaml)?;
    validate_governance_state(".rct/plan.yaml", plan_yaml)?;
    validate_governance_state(".rct/checklist.yaml", checklist_yaml)?;
    validate_traceability_statuses(traceability_markdown)?;
```

These four calls are the entire validator; the CLI calls this same function and then reports success.

**`meerkat-mobkit/tests/governance_contracts.rs:173-190`**

```text
status: MISSING
    evidence: []
```

A deliberately unimplemented row is an accepted format/status fixture, corroborating the preserved limitation.

## E-012: Baseline symbol scanning is falsely presented as crate/export/version compatibility verification

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/reference/governance.mdx:58-65`**

```text
The baseline check ensures that:
- Required Meerkat crates are present
- Expected API symbols are exported
- Version compatibility is maintained
```

A passing heuristic scan is mistaken for dependency compatibility assurance and may be used instead of the actual pinned-dependency build/check.

**`meerkat-mobkit/src/baseline.rs:139-148`**

```text
missing.retain(|symbol| !contains_symbol(&content, symbol));
```

The verifier recursively reads text and removes tokens when they occur. It does not parse Cargo manifests, check Rust export visibility, type-check signatures or compare dependency versions.

**`meerkat-mobkit/src/baseline.rs:170-181`**

```text
if content.contains(symbol) {
        return true;
    }
```

Literal token occurrences, including comments, satisfy this historical heuristic.

**`meerkat-mobkit/src/baseline.rs:255-265`**

```text
repo.path().join("baseline-symbols.rs"),
            REQUIRED_MEERKAT_SYMBOLS.join("\n"),
```

The unit test creates only a file containing the token list and then asserts a successful report. No crates, valid Rust exports or version manifest exist in that fixture. REQUIRED_MEERKAT_SYMBOLS is a selected historical checklist, not every entry point imported by current MobKit.

### Independent adjudication

This is not merely a lightweight version checker: it does not read versions or prove crates/exports at all. It recursively scans text for selected tokens and accepts relaxed token alternatives; even comments can satisfy it. The existing success fixture contains only the checklist text and no manifest or compilable exports. The path/MEERKAT_REPO guidance is already correct and should remain untouched.

**`meerkat-mobkit/src/baseline.rs:139-148`**

```text
missing.retain(|symbol| !contains_symbol(&content, symbol));
```

Symbol satisfaction is based on reading strings from files, not compiling crates or inspecting public APIs.

**`meerkat-mobkit/src/baseline.rs:172-178`**

```text
if content.contains(symbol) {
```

Any literal occurrence passes before the special-case relaxed matching logic.

**`meerkat-mobkit/src/baseline.rs:254-267`**

```text
REQUIRED_MEERKAT_SYMBOLS.join("\n"),
```

The test writes only the symbol checklist into a new directory and expects verification success, proving crates, exports and versions are not prerequisites.

**Required correction:** Call this an optional source-token smoke check against a selected historical checklist. Replace the guarantees of crate presence, exported API/signature correctness, version compatibility and complete dependency-entry-point coverage with its actual heuristic scope. Direct compatibility assurance to pinned dependency manifests and compilation/tests while preserving explicit path and MEERKAT_REPO instructions.

### Changes and final verification

**Changed:** `docs/reference/governance.mdx`.

Described baseline verification as an optional heuristic source-token scan over a selected historical checklist. Removed guarantees of crate presence, exports/signatures, version compatibility, and exhaustive dependency coverage. Preserved explicit-path/MEERKAT_REPO guidance and directed compatibility checks to pinned manifests and compilation/tests.

**Validation:** Read baseline.rs:63-195 and checked the checklist-only successful test fixture at 254-267. The documented limitations match literal/relaxed token matching; no upstream checkout or scan was required.

**Final review: pass.** Baseline verification is correctly framed as an optional heuristic source-token scan over a selected historical checklist. Crate, public-export/signature, version, and exhaustive-entry-point guarantees are gone. The already-correct explicit-path/MEERKAT_REPO behavior is preserved, and the new imported example matches exported types and signature.

**`docs/reference/governance.mdx:68-83`**

```text
This is a heuristic source-token smoke check: even occurrences in comments
can satisfy a checklist entry. It does not establish crate presence, exported
API or signature correctness, or version compatibility.
```

This states the actual strength of the scan and directs compatibility work to manifests and compilation/tests.

**`docs/reference/governance.mdx:87-89`**

```text
`REQUIRED_MEERKAT_SYMBOLS` contains that selected historical checklist, not
an exhaustive inventory of MobKit's current dependency entry points.
```

The misleading completeness claim is removed separately from the scan-result guarantees.

**`meerkat-mobkit/src/baseline.rs:172-178`**

```text
if content.contains(symbol) {
        return true;
    }
```

Literal text, including comments, satisfies an entry before relaxed alternatives are considered.

**`meerkat-mobkit/src/baseline.rs:254-267`**

```text
REQUIRED_MEERKAT_SYMBOLS.join("\n"),
```

The successful source fixture contains only checklist text, not a compilable crate/export/version proof. This test was inspected, not executed.

## E-013: Delivery validation omits route-cache expiry and the retained-idempotency replay exception

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/delivery.mdx:18-22`**

```text
The runtime requires the supplied resolution to equal the retained resolution for its `route_id`. Unknown or modified resolutions are rejected before dispatch.
```

Clients cannot tell when they must re-resolve an old route and may incorrectly discard a still-valid idempotent retry after route eviction.

**`meerkat-mobkit/src/runtime.rs:1460-1462`**

```text
const ROUTING_RESOLUTION_LIMIT_MAX: usize = 512;
```

Trusted routing resolutions have a separate bounded cache, not indefinite retention until the delivery-history entry expires.

**`meerkat-mobkit/src/runtime/routing.rs:124-132`**

```text
while self.routing_resolution_order.len() > ROUTING_RESOLUTION_LIMIT_MAX {
            let oldest_route_id = self.routing_resolution_order.remove(0);
            self.routing_resolutions.remove(&oldest_route_id);
        }
```

A previously issued unused route can become unknown after more resolutions even within one running instance.

**`meerkat-mobkit/src/runtime/delivery.rs:136-153`**

```text
return Ok(existing);
```

send_delivery attempts keyed replay before trusted_resolution_for_delivery. Replay compares the cached canonical_resolution when the route cache no longer contains the ID, so an evicted ID is not unconditionally rejected.

**`meerkat-mobkit/tests/routing_delivery.rs:1204-1218`**

```text
replay_after_eviction["result"]["delivery_id"],
        first_send["result"]["delivery_id"]
```

The existing route-cache-eviction test verifies that a new send on the evicted route fails, a forged replay fails, but a matching retained idempotent replay returns the original record. This test is marked #[ignore]; the production branch is the primary proof.

### Independent adjudication

Confirmed narrowly as a missing route lifetime and misleading validation-order contract, not as a bypass of dispatch validation. The literal 'before dispatch' remains true for new sends: replay performs no dispatch. However, the flow presents retained-route validation before the replay exception and gives only the delivery-history retention bound. The independent 512-resolution cache can invalidate a never-used route while a matching existing keyed replay still succeeds. That distinction changes client retry/re-resolve behavior and is not a stylistic preference.

**`meerkat-mobkit/src/runtime.rs:1459-1467`**

```text
const ROUTING_RESOLUTION_LIMIT_MAX: usize = 512;
```

Issued route retention has its own hard bound, distinct from delivery history.

**`meerkat-mobkit/src/runtime/routing.rs:124-133`**

```text
self.routing_resolutions.remove(&oldest_route_id);
```

Resolving more routes evicts the oldest trusted resolution regardless of whether it has been dispatched.

**`meerkat-mobkit/src/runtime/delivery.rs:87-103`**

```text
} else if entry.canonical_resolution != *provided_resolution {
```

Replay validates against its retained canonical resolution when the issued-route cache no longer contains the route.

**`meerkat-mobkit/src/runtime/delivery.rs:140-152`**

```text
return Ok(existing);
```

The replay branch returns before trusted_resolution_for_delivery, so a matching cached delivery does not need a still-retained issued route.

**`meerkat-mobkit/tests/routing_delivery.rs:1203-1218`**

```text
replay_after_eviction["result"]["delivery_id"],
```

The ignored historical regression contrasts rejection of a new send/forged replay with return of the original matching delivery. Source, not an executed test result, is the authority here.

**Required correction:** Explain keyed replay first, then validation for new dispatch. Document the 512-issued-resolution cache and that a new send on an evicted route must resolve again. A matching idempotent replay may outlive route-cache eviction through the delivery's canonical resolution, only while its delivery/idempotency record is retained and ordinary prerequisites such as the loaded delivery module still hold. Preserve the existing guarantee that forged resolutions cannot dispatch.

### Changes and final verification

**Changed:** `docs/concepts/delivery.mdx`.

Reordered the flow to keyed replay before new-dispatch trusted-route validation. Documented the separate 512-resolution cache, the need to re-resolve evicted routes for new sends, and canonical-resolution replay surviving eviction only while its delivery/idempotency record is retained and the delivery module remains loaded. Preserved forged-resolution rejection and qualified history-eviction retry behavior.

**Validation:** Read runtime/delivery.rs:47-60,74-157 and runtime/routing.rs:124-133; automated assertion verified ROUTING_RESOLUTION_LIMIT_MAX = 512. Adjudicated historical test evidence was checked as source only, not executed.

**Final review: pass.** The flow now checks retained keyed replay before new-dispatch trusted-route validation. The independent 512-resolution and 200-delivery bounds are correct. Canonical replay may survive route eviction only while its record and idempotency entry remain and the delivery module is loaded; new sends must re-resolve evicted routes. Forged resolutions and mismatched payloads remain explicitly rejected.

**`docs/concepts/delivery.mdx:110-116`**

```text
Issued resolutions have a separate cache of at most 512 entries. A new
dispatch on an evicted route must resolve again.
```

The previously omitted issued-route lifetime is documented next to bounded replay.

**`meerkat-mobkit/src/runtime/delivery.rs:74-103`**

```text
} else if entry.canonical_resolution != *provided_resolution {
```

A matching retained idempotency entry authenticates replay even after issued-route eviction.

**`meerkat-mobkit/src/runtime/delivery.rs:125-152`**

```text
return Ok(existing);
```

The return precedes trusted_resolution_for_delivery, but follows the loaded-delivery-module and key checks.

**`meerkat-mobkit/src/runtime.rs:1459-1466`**

```text
const ROUTING_RESOLUTION_LIMIT_MAX: usize = 512;
```

The documented bound is the actual runtime constant, distinct from the 200-record history bound.

## E-014: Retry and attempt documentation omits that attempts are synthesized rather than executed

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/delivery.mdx:124-124`**

```text
`DeliveryRecord` contains the delivery and route IDs, recipient, sink, target module, original payload, status, attempt records, first and final attempt timestamps, optional idempotency key, and optional `sink_adapter`.
```

Operators can read attempt history as observed network failures or rely on retry_max/backoff_ms to retry a failed delivery, neither of which the runtime does.

**`meerkat-mobkit/src/runtime/delivery.rs:191-212`**

```text
let mcp_response = call_module_mcp_tool_json(
```

There is one delivery MCP invocation before attempt construction; boundary failure returns immediately through map_err(...)? rather than entering a retry loop.

**`meerkat-mobkit/src/runtime/delivery.rs:233-249`**

```text
let total_attempts = trusted_resolution.retry_max.saturating_add(1);
        for attempt in 1..=total_attempts {
```

This loop only pushes DeliveryAttempt records. Every nonfinal entry is unconditionally labeled transient_failure; there is no repeated send and no sleep.

**`meerkat-mobkit/src/runtime/delivery.rs:251-257`**

```text
let final_attempt_ms = first_attempt_ms.saturating_add(
            trusted_resolution
                .backoff_ms
                .saturating_mul(u64::from(trusted_resolution.retry_max)),
        );
```

The final timestamp is arithmetic based on configured backoff, not an observed completion time for actual repeated attempts.

**`meerkat-mobkit/tests/routing_delivery.rs:486-529`**

```text
{"attempt":1,"status":"transient_failure","backoff_ms":125},
```

The existing deterministic retry_max=3 test expects synthetic transient failures even for the successful call. It is an ignored historical test, used only as corroboration of the production loop.

### Independent adjudication

The record inventory is structurally correct, but the guide exposes retry_max/backoff_ms and 'attempt records' without the material qualification that these are synthetic. I checked the MCP helper as well as send_delivery to rule out a hidden retry loop: the helper lists tools and calls connection.call_tool once; its error propagates. The later attempt loop only pushes records and computes timestamps. This is a documentation limitation to disclose, not authorization to change dispatch semantics.

**`meerkat-mobkit/src/runtime/module_boundary.rs:220-242`**

```text
let blocks = timeout(timeout_duration, connection.call_tool(tool_name, args))
```

The MCP tool boundary performs one tool invocation under timeout, with no retry loop.

**`meerkat-mobkit/src/runtime/delivery.rs:196-214`**

```text
.map_err(DeliverySendError::DeliveryBoundary)?;
```

The sole delivery MCP invocation's failure exits before attempt-record construction.

**`meerkat-mobkit/src/runtime/delivery.rs:233-257`**

```text
"transient_failure".to_string()
```

Every nonfinal row is assigned this status mechanically; the loop contains neither another dispatch nor a sleep, and final_attempt_ms is derived arithmetically.

**`meerkat-mobkit/tests/routing_delivery.rs:484-531`**

```text
{"attempt":4,"status":"sent","backoff_ms":0}
```

The ignored deterministic test expects three artificial transient failures then success for retry_max=3 and a computed 375ms span; it does not count actual retries.

**Required correction:** Add a prominent current limitation near the retry fields and DeliveryRecord: each non-replayed send invokes delivery.send once; after a successful boundary call MobKit synthesizes retry_max+1 attempt rows and a backoff-derived final timestamp. They are not observations of repeated dispatches or elapsed backoff. Required real retries/backoff must be implemented by the application delivery module or host policy; do not change runtime code in this wave.

### Changes and final verification

**Changed:** `docs/concepts/delivery.mdx`.

Added a prominent retry-field limitation and record-level explanation: dispatch calls delivery.send once; after boundary success, attempt rows and final timestamps are synthesized rather than observed retries or elapsed backoff. Boundary errors return immediately. Real retries remain the application delivery module or host's responsibility.

**Validation:** Read runtime/delivery.rs:191-257 and verified the single-call MCP boundary excerpt at runtime/module_boundary.rs:220-242. The retry cap is 10 in routing.rs/runtime.rs, so the documented retry_max + 1 count is consistent with accepted resolutions. No runtime retry changes were made.

**Final review: pass.** A prominent warning and the record section consistently distinguish one actual MCP call from synthetic retry rows and arithmetic timestamps. The retry_max+1 count, final status, transient_failure labels, saturating timestamp formula, and immediate boundary-error propagation all match source. Real retry responsibility is assigned to the host/module without changing runtime behavior.

**`docs/concepts/delivery.mdx:56-63`**

```text
`retry_max` and `backoff_ms` currently shape synthetic attempt records; they
do not cause MobKit to retry the MCP call or wait between attempts.
```

The operational limitation appears where clients first encounter retry fields.

**`docs/concepts/delivery.mdx:143-150`**

```text
`final_attempt_ms` is computed
from `first_attempt_ms + backoff_ms * retry_max` (with saturating arithmetic),
not measured after repeated sends.
```

The new numerical explanation accurately distinguishes record construction from elapsed time.

**`meerkat-mobkit/src/runtime/module_boundary.rs:220-242`**

```text
let blocks = timeout(timeout_duration, connection.call_tool(tool_name, args))
```

Following the helper confirms one call_tool invocation, not a hidden retry loop.

**`meerkat-mobkit/src/runtime/delivery.rs:233-257`**

```text
let total_attempts = trusted_resolution.retry_max.saturating_add(1);
```

The later loop only constructs attempt records; its timestamp uses saturating addition/multiplication and contains neither dispatch nor sleep.

## E-015: Delivery timestamp ordering is only synchronized to merged events during route resolution

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/delivery.mdx:138-138`**

```text
Routing and delivery timestamps use a runtime-relative monotonic floor that is also advanced to the latest merged-event timestamp. A route or delivery inserted after an existing merged event therefore does not receive an earlier timestamp.
```

Consumers are promised insertion-time ordering that is not provided when live agent events arrive between resolve and send; a delivery may be sorted before a previously merged event.

**`meerkat-mobkit/src/runtime/delivery.rs:27-43`**

```text
let latest_merged_timestamp_ms = self
            .merged_events
            .last()
            .map_or(0, |event| event.timestamp_ms);
```

Only next_route_resolved_timestamp_ms takes the latest merged-event maximum. refresh_delivery_clocks_from_now uses runtime-relative elapsed time and the previous delivery clock.

**`meerkat-mobkit/src/runtime/delivery.rs:186-190`**

```text
let first_attempt_ms = self
            .delivery_clock_ms
            .saturating_add(DELIVERY_CLOCK_STEP_MS);
```

send_delivery advances the prior clock by 1000 ms and does not read the latest merged event. With retry_max=0, this is also the final timestamp.

**`meerkat-mobkit/src/runtime/event_transport.rs:52-60`**

```text
insert_event_sorted(&mut self.merged_events, event);
```

Appending a new agent event does not advance delivery_clock_ms.

**`meerkat-mobkit/src/unified_runtime/lifecycle.rs:723-728`**

```text
.append_normalized_event(unified_event)?;
```

The normal live event drain uses that append path. Concrete counterexample: resolve at absolute timestamp T, then drain a later agent event at T+10000, then send on the retained route with retry_max=0 while the runtime-relative floor remains below T. Delivery receives T+1000, earlier than the already-merged event. This is a source-level counterexample, not an executed runtime test.

### Independent adjudication

I challenged the broad ordering claim against the public event-ingress path and the timestamp tests. Only route resolution samples the latest merged timestamp; appending a live agent event neither rewrites its timestamp nor advances the delivery clock. Existing tests cover resolve after prior events and resolve after send, not a later agent event between resolve and send, and are ignored. A retained route resolved at T, followed by an appended event at T+10000 and an immediate retry_max=0 send while the elapsed floor is lower, produces a delivery at T+1000. This is a source-level counterexample, not a claimed executed regression.

**`meerkat-mobkit/src/runtime/delivery.rs:27-42`**

```text
let timestamp_ms = self.delivery_clock_ms.max(latest_merged_timestamp_ms);
```

The merged-event synchronization occurs in next_route_resolved_timestamp_ms, whereas refresh_delivery_clocks_from_now uses only elapsed wall time and the existing floor.

**`meerkat-mobkit/src/runtime/delivery.rs:165-190`**

```text
.saturating_add(DELIVERY_CLOCK_STEP_MS);
```

send_delivery advances its previous clock by the 1000ms constant without sampling merged_events.

**`meerkat-mobkit/src/runtime/event_transport.rs:52-60`**

```text
insert_event_sorted(&mut self.merged_events, event);
```

Live append only validates source consistency and sorts the event; it does not update a delivery clock.

**`meerkat-mobkit/src/unified_runtime/lifecycle.rs:721-728`**

```text
.append_normalized_event(unified_event)?;
```

The ordinary event-drain path reaches that append function, making the between-resolve-and-send case reachable.

**`meerkat-mobkit/tests/routing_delivery.rs:1046-1106`**

```text
>= first_send["result"]["final_attempt_ms"]
```

The apparently relevant monotonicity test asserts a subsequent resolution follows a delivery; it does not establish the guide's stronger send-after-any-live-event guarantee.

**Required correction:** State that route resolution advances the delivery clock to the latest merged-event timestamp and subsequent delivery timestamps advance that clock. Explicitly exclude live events appended after the resolution from any insertion-order guarantee: send_delivery does not independently incorporate their timestamps. Keep synthetic attempt-time limitations consistent with E-014 and do not modify runtime timing behavior.

### Changes and final verification

**Changed:** `docs/concepts/delivery.mdx`.

Limited merged-event clock synchronization to route resolution and explained subsequent delivery-clock advancement. Explicitly warned that live events appended after resolution are not independently sampled by send_delivery, so timestamps do not guarantee insertion order across all event sources.

**Validation:** Read runtime/delivery.rs:27-43,165-190 and checked event_transport.rs:52-60 plus lifecycle.rs:721-728 through exact adjudicated evidence. The documented interleaving limitation is source-derived, not an executed race test.

**Final review: pass.** The clock contract is now scoped to synchronization at route resolution and subsequent advancement of that delivery clock. It explicitly excludes later live events from a universal insertion-order guarantee and remains consistent with synthetic attempt time. The counterexample is source-derived, not falsely presented as an executed timing test.

**`docs/concepts/delivery.mdx:168-173`**

```text
`send_delivery` does not independently incorporate timestamps from live
events appended after route resolution.
```

The original unconditional ordering promise is removed.

**`meerkat-mobkit/src/runtime/delivery.rs:27-42`**

```text
let timestamp_ms = self.delivery_clock_ms.max(latest_merged_timestamp_ms);
```

Only next_route_resolved_timestamp_ms samples the latest merged event; ordinary clock refresh uses elapsed time and existing floors.

**`meerkat-mobkit/src/runtime/event_transport.rs:52-60`**

```text
insert_event_sorted(&mut self.merged_events, event);
```

The live append path sorts the incoming event without advancing the delivery clock; send_delivery likewise does not resample merged_events.

## E-016: Event-line module calls require process exit, not merely the first JSON line

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/guides/module-system.mdx:85-91`**

```text
The runtime normalizes that line into `EventEnvelope<UnifiedEvent>`. On this
boundary, `route_module_call` launches the module and returns the first
normalized module event's `payload` field as `ModuleRouteResponse.payload`.
```

The guide's startup section explicitly permits retained healthy children, while its routed-call section does not warn that using such a long-lived event-line process can hang forever after emitting its event.

**`meerkat-mobkit/src/runtime/routing.rs:64-68`**

```text
let envelope = run_module_boundary_once(module, pre_spawn, timeout)
```

Generic routed calls go through the one-shot process helper, not the startup-supervision helper that retains a healthy child.

**`meerkat-mobkit/src/process.rs:60-69`**

```text
Ok((Ok(_), mut line)) => {
            wait_with_context(
                &mut child,
                "failed to wait for process after reading output",
            )?;
```

After a valid first line has arrived, the helper waits for the process to exit before returning or normalizing the line.

**`meerkat-mobkit/src/process.rs:96-100`**

```text
child
        .wait()
```

The exit wait is unbounded. The supplied timeout only bounds waiting for the first line, so a daemon that writes a readiness line then stays alive hangs a routed call despite its timeout.

### Independent adjudication

The guide does call a different helper 'one-shot', but it never imposes termination on the generic routed call, immediately alongside startup policies that retain healthy children. Tracing route_module_call through supervisor.rs reaches run_process_json_line, which waits without a deadline after the first line arrives. Thus emitting a readiness line is insufficient for a routed event-line call to complete. The existing timeout regression sleeps without emitting anything, so it tests first-line timeout only and cannot disprove the hang after output.

**`meerkat-mobkit/src/runtime/routing.rs:64-77`**

```text
let envelope = run_module_boundary_once(module, pre_spawn, timeout)
```

Generic routing uses the one-shot boundary, not the retained-child startup supervisor.

**`meerkat-mobkit/src/runtime/supervisor.rs:14-24`**

```text
let line = run_process_json_line(&module.command, &module.args, &env, timeout)
```

The helper directly selects the process implementation whose exit wait is in question.

**`meerkat-mobkit/src/process.rs:59-100`**

```text
"failed to wait for process after reading output",
```

The successful first-line branch invokes wait_with_context, implemented as child.wait(), after recv_timeout has already finished. There is no deadline around that wait.

**`meerkat-mobkit/tests/external_boundary.rs:94-104`**

```text
let module = shell_module("sleepy", "sleep 5");
```

The ignored timeout test covers no output, not output followed by a long-running child.

**Required correction:** Explicitly require event-line route_module_call/run_module_boundary_once subprocesses to emit their first JSON line and terminate. Say the timeout bounds first-line arrival, not the subsequent child-exit wait. Contrast this with startup supervision retaining a healthy child. Recommend the MCP boundary for request/response servers, while preserving the fact that each MCP routed call connects and closes rather than promising connection reuse.

### Changes and final verification

**Changed:** `docs/guides/module-system.mdx`.

Required routed event-line subprocesses to emit their first JSON line and terminate. Documented that the timeout bounds first-line arrival, not the subsequent unbounded child-exit wait, and contrasted this with startup retaining a healthy child. Recommended MCP for request/response servers without promising persistent connection reuse.

**Validation:** Read runtime/routing.rs:64-77 and process.rs:33-100; checked supervisor.rs:14-24 through exact adjudicated evidence. MDX component and JSON example checks passed. No hanging subprocess was launched.

**Final review: pass.** The generic routed event-line contract now requires both output and child termination. It correctly distinguishes first-line timeout from the later unbounded exit wait and contrasts that behavior with retained-child startup supervision. MCP is recommended without inventing connection reuse, and the existing no-stdin-forwarding and normalized-payload caveats remain intact.

**`docs/guides/module-system.mdx:96-103`**

```text
execution paths. Their timeout bounds first-line arrival, not the subsequent
wait for child exit. A subprocess that emits a readiness line and stays alive
can leave the call waiting indefinitely.
```

The formerly hidden completion condition is now an explicit warning.

**`meerkat-mobkit/src/process.rs:59-100`**

```text
"failed to wait for process after reading output",
```

After recv_timeout succeeds, the code calls wait_with_context, whose child.wait has no deadline.

**`meerkat-mobkit/src/runtime/routing.rs:64-77`**

```text
let envelope = run_module_boundary_once(module, pre_spawn, timeout)
```

The generic routed path reaches that same one-shot process implementation.

**`meerkat-mobkit/src/runtime/module_boundary.rs:100-130`**

```text
let close_result = close_with_timeout(&module_id, connection, timeout_duration).await;
```

Each routed MCP operation owns and closes its connection; the revised recommendation preserves that limitation.

## E-017: Unified bootstrap sequence incorrectly promises member provisioning before modules start

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/guides/unified-runtime.mdx:56-57`**

```text
The Meerkat mob runtime starts first. Members are provisioned, wiring is established, and the mob reaches an operational state. If this fails, the builder returns `UnifiedRuntimeBootstrapError::Mob`.
```

Library embedders expect the minimal definition-based example to provision agents and expect module startup to observe an already-populated/wired mob, which is not guaranteed.

**`meerkat-mobkit/src/unified_runtime/mod.rs:694-704`**

```text
let (mob_runtime, pending_mob_activation) = MobRuntime::prepare(mob_spec)
```

The runtime prepares the mob, then immediately starts modules on the next thread. Identity-first persistent resumes may deliberately remain Stopped pending later identity registration.

**`meerkat-mobkit/src/unified_runtime/builder.rs:1504-1540`**

```text
if runtime.identity_first_context.is_none()
            && let Some(ref discovery) = runtime.discovery
```

Identity bootstrap or classic Discovery member spawning occurs late in builder construction, after the mob/module runtime already exists. Definition-only builds with no roster/discovery do not automatically spawn profile members.

**`meerkat-mobkit/src/unified_runtime/builder.rs:1550-1557`**

```text
// Run initial edge reconciliation after spawn completes
```

The final builder edge reconciliation is after these optional member-spawn phases, not an unconditional completed action in the first bootstrap step.

**`meerkat-mobkit/tests/runtime_bootstrap.rs:100-111`**

```text
assert!(handle.list_members_including_retiring().await.is_empty());
```

The existing bootstrap test expressly distinguishes a Running mob from having any members, then spawns one explicitly. It is #[ignore], so this corroborates the production ordering rather than claiming an executed test.

### Independent adjudication

The claimed unconditional pre-module member provisioning is disproved by builder ordering, not merely a missing example. Unified bootstrap prepares the mob, starts modules, and returns to the builder; identity roster bootstrap or classic Discovery spawning happens later when configured, followed by final edge reconciliation. Some persistent identity resumes are intentionally still Stopped during module startup. This does not mean all mobs always start empty: persisted members or configured later roster/discovery can populate them. The correction must preserve that distinction.

**`meerkat-mobkit/src/unified_runtime/mod.rs:691-704`**

```text
let (mob_runtime, pending_mob_activation) = MobRuntime::prepare(mob_spec)
```

Module startup follows prepare, whose activation may deliberately be deferred, not unconditional fully populated Running state.

**`meerkat-mobkit/src/unified_runtime/builder.rs:1504-1557`**

```text
if let Err(err) = context.bootstrap_roster(roster_specs).await {
```

The late builder phase bootstraps configured identities, alternatively spawns classic Discovery results, and then performs final configured edge reconciliation.

**`meerkat-mobkit/src/mob_handle_runtime.rs:8360-8382`**

```text
/// Build the mob without committing to `Running`.
```

The prepare/activate split is an explicit authority contract, not a transient implementation accident.

**`meerkat-mobkit/tests/runtime_bootstrap.rs:100-111`**

```text
assert!(handle.list_members_including_retiring().await.is_empty());
```

The ignored bootstrap regression contrasts a Running mob with no members, then explicitly spawns one. It corroborates source ordering without supplying an executed test result.

**Required correction:** Rewrite the sequence as mob preparation, module startup, then configured identity roster bootstrap or classic Discovery member spawning and final edge reconciliation in the builder. Explain that profiles are templates and a fresh definition-only runtime without roster/discovery may have no members. Qualify Running/operational claims for staged persistent identity activation, and do not imply persisted members are always absent.

### Changes and final verification

**Changed:** `docs/guides/unified-runtime.mdx`.

Rewrote bootstrap ordering as mob preparation, module startup, then configured identity roster or classic Discovery materialization and final configured edge reconciliation. Qualified staged Stopped identity resumes, profiles-as-templates, potentially empty fresh definition-only runtimes, and pre-existing persisted members. Updated adjacent rollback wording consistently.

**Validation:** Read unified_runtime/mod.rs:691-740 and builder.rs:1495-1557; the sequence follows actual preparation/startup/materialization calls. Local links and MDX step pairing passed.

**Final review: pass.** The sequence now separates mob preparation, module startup, and configured later member materialization/reconciliation. It preserves staged Stopped identity resumes and does not claim every bootstrap is empty. Profiles remain templates, and definition-only hosts are warned that membership is not automatic. Rollback wording is consistent with preparation rather than unconditional prior activation.

**`docs/guides/unified-runtime.mdx:55-64`**

```text
Later in builder construction, configured identity roster bootstrap or classic `Discovery` spawning materializes the desired members.
```

The old pre-module member-provisioning guarantee has been replaced with the real builder ordering.

**`docs/guides/unified-runtime.mdx:70-74`**

```text
Persistent resumes can already have
members, so this is not a guarantee that every bootstrap starts empty.
```

The correction retains the necessary persisted-member caveat.

**`meerkat-mobkit/src/unified_runtime/mod.rs:691-704`**

```text
let (mob_runtime, pending_mob_activation) = MobRuntime::prepare(mob_spec)
```

prepare precedes start_mobkit_runtime_with_options; the staged-activation branch defers initial reconciliation when appropriate.

**`meerkat-mobkit/src/unified_runtime/builder.rs:1504-1557`**

```text
if let Err(err) = context.bootstrap_roster(roster_specs).await {
```

The later builder phase bootstraps identities, otherwise spawns classic Discovery specs, and finally reconciles configured edges.

## E-018: Operational error-event reference omits the terminal actor-loop event and its restart consequence

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/guides/unified-runtime.mdx:296-312`**

```text
| `actor_loop_stalled` | `probe_waited_secs`, `detail`, `stall_id?`, `prior_resolved_stalls?` | the actor-loop probe's round trip went unanswered past its budget |
| `actor_loop_recovered` | `stall_id`, `stalled_for_secs` | that stall resolved; the one non-failure variant, correlated by `stall_id` |
```

A host implementing paging from the supposedly current event inventory handles stalls/recovery but misses the distinct terminal failure requiring process restart, or waits forever for a recovery event that will not arrive.

**`meerkat-mobkit/src/unified_runtime/types.rs:619-638`**

```text
ActorLoopTerminated {
```

The current public non-exhaustive enum also includes ActorLoopTerminated with optional stall_id and required detail; snake_case serialization produces actor_loop_terminated.

**`meerkat-mobkit/src/unified_runtime/mod.rs:2643-2664`**

```text
ErrorEvent::ActorLoopTerminated {
                            stall_id: Some(stall_id),
                            detail,
                        },
```

A closed parked actor channel emits termination, marks health terminated, and stops the probe instead of producing actor_loop_recovered. The adjacent production diagnostic says the process must restart.

**`meerkat-mobkit/src/unified_runtime/mod.rs:3771-3776`**

```text
"a dead actor must NEVER be reported as recovered: {events:?}"
```

The current paused-time test explicitly verifies this distinction and checks that no further probes run. This is materially different operator behavior, not merely another optional field.

### Independent adjudication

The wildcard example is already forward-compatible, but that does not make a missing current terminal event harmless in the operator-facing event inventory. The enum, both fresh and parked probe branches, and dedicated tests distinguish termination from recovery and stop probing. I also checked the bridge rather than relying only on the enum comment: terminated health yields ActorTerminated and instructs restart. The required documentation addition is concrete operator behavior, not a request to enumerate unknown future variants.

**`meerkat-mobkit/src/unified_runtime/types.rs:619-638`**

```text
ActorLoopTerminated {
```

The public event variant has detail plus optional stall_id; the enum's snake_case category serialization produces actor_loop_terminated.

**`meerkat-mobkit/src/unified_runtime/mod.rs:2640-2664`**

```text
health.mark_terminated(Some(stall_id), detail.clone());
```

A closed parked round trip marks terminal health, fires ActorLoopTerminated and breaks instead of sending a recovery.

**`meerkat-mobkit/src/unified_runtime/mod.rs:3770-3799`**

```text
"a dead actor must NEVER be reported as recovered: {events:?}"
```

The paused-time unit regression asserts no recovery, terminal shared health, exactly one probe, and a finished probe task. The separate fresh-probe regression covers stall_id: None.

**`meerkat-mobkit/src/identity_first/bridge.rs:1517-1530`**

```text
Some(BridgeError::ActorTerminated {
```

The guarded delivery path refuses terminal actor health rather than waiting for in-process recovery.

**Required correction:** Add actor_loop_terminated to the table with detail and optional stall_id. Explain that command/reply channel closure means the actor is gone, the probe ends, guarded deliveries fail fast with ActorTerminated, and recovery requires restarting the process. Explicitly distinguish it from actor_loop_recovered and allow termination without a prior stall.

### Changes and final verification

**Changed:** `docs/guides/unified-runtime.mdx`.

Added actor_loop_terminated with detail and optional stall_id to the event inventory. Explained termination without a prior stall, end of probing, guarded delivery's ActorTerminated fast failure, and required process restart rather than an in-process recovery notification.

**Validation:** Read unified_runtime/types.rs:619-638 and mod.rs:2640-2664; exact evidence checks also covered the no-false-recovery regression and identity_first/bridge.rs:1517-1530. The event remains distinct from actor_loop_recovered.

**Final review: pass.** actor_loop_terminated is added with detail and optional stall_id. The guide correctly distinguishes terminal failure from recovery, allows termination without a prior stall, says probing ends, and describes guarded ActorTerminated delivery failure and process restart. The forward-compatible wildcard/non-exhaustive advice remains unchanged.

**`docs/guides/unified-runtime.mdx:322-333`**

```text
It can occur without a prior stall (`stall_id` is then absent). The probe
ends, guarded deliveries fail fast with `ActorTerminated`, and there is no
in-process actor recovery.
```

The operational restart consequence and fresh-probe case are explicitly documented.

**`meerkat-mobkit/src/unified_runtime/types.rs:619-638`**

```text
ActorLoopTerminated {
```

The variant contains detail and optional, omission-serialized stall_id and explicitly requires restart.

**`meerkat-mobkit/src/unified_runtime/mod.rs:2680-2698`**

```text
health.mark_terminated(None, detail.clone());
```

The fresh-probe branch emits termination with no prior stall and breaks; the parked branch separately preserves the existing stall ID.

**`meerkat-mobkit/src/identity_first/bridge.rs:1517-1530`**

```text
Some(BridgeError::ActorTerminated {
```

The delivery guard actually refuses terminal health instead of awaiting recovery.

## Independent scope checks

> [
>   "Read the coordination brief, scope map and all 18 audit-E findings; independently inspected all nine assigned documentation files.",
>   "Traced disputed behavior in current local implementation and inspected relevant Rust/Python/TypeScript regression tests, including counter-evidence and test scope.",
>   "Read-only git rev-parse HEAD matched af82b6b3ab34faed9bf3e962d148d55f10dcd1dc.",
>   "Read-only Python validation passed: valid JSON, exactly one decision for each of 18 finding IDs, valid verdict/correction fields, and all 63 quoted evidence excerpts present within their cited source-line ranges.",
>   "No repository files, source behavior, git state, or dependency installations changed; only this requested adjudication artifact was written."
> ]

## Final scope checks

> [
>   "Read audit-brief.md, audit-scopes.json, audit-E.json, adjudication-E.json, fixes-E.json, audit-K.json/adjudication-K.json K-005/K-006, and fixes-coordination.json. There is no fixes-K.json: K ownership is distributed as the brief specifies, and E's two portions are recorded in fixes-E.json.",
>   "Read all nine E-owned documents in full, the complete six-file E diff, the coordinated Rust SDK governance correction, and the coordinated Python/TypeScript RosterContext documentation diffs. Inspected actual production implementation and relevant existing tests rather than relying on fix-report assertions.",
>   "Ledger validation: exactly 18 E finding IDs, 18 confirmed decisions, 18 E fixes, and two assigned K fixes. Zero rejected E findings and zero supplemental E findings.",
>   "Independent exact-evidence check: 120 implementation/test excerpts from audit-E/adjudication-E remain present within their cited source ranges. Production behavior was independently traced around the relevant excerpts.",
>   "Independent schema checks passed: all five SessionPersistenceRow fields match the corrected table, all six GovernanceValidationError and four BaselineVerificationError variants are covered, and all three no-finding E documents (gating/modules/scheduling) remain unchanged.",
>   "git diff --check passed for all nine E-owned documents and docs/sdks/rust.mdx.",
>   "Used the existing session-installed @mdx-js/mdx compiler with remark-frontmatter: 10/10 passed for all nine E pages plus docs/sdks/rust.mdx. No dependency installation or repository output was needed.",
>   "Independent Python stdlib checks passed: 40 local documentation links/anchors resolve, three Python snippets parse, and seven JSON snippets parse (the labeled role_migrations property fragment was checked with enclosing braces). No E document is a symlink.",
>   "Attempted focused pytest roster-callback regression selection; the installed Python has no pytest. Without installing dependencies, directly exercised the actual CallbackDispatcher with PYTHONDONTWRITEBYTECODE=1: nested typed definition/previous identities, typed empty-context fallback, and absent-context fallback all passed, with valid returned DurableAgentSpec serialization.",
>   "K-005 coordination preservation check passed: Python ASTs are identical after removing docstrings, and TypeScript source is identical after removing block comments. The coordinated source-file changes introduce no executable behavior change.",
>   "Rust examples were independently checked against complete function bodies, public signatures, and crate-root exports. In particular, both governance examples have valid document shapes; the new constructors use public APIs and BigQuery host variables are explicitly introduced by prose.",
>   "No repository edits, git mutations, dependency installs, external service calls, commits, or delegation were performed by this reviewer. Only the requested review artifact was created."
> ]
