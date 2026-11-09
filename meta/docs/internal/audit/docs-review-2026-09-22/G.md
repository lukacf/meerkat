# G: Memory documentation, architecture, calibration and runtime prompts

[Audit index](README.md) | [Coverage](coverage.md)

Original documentation and initial evidence line ranges refer to baseline `af82b6b3ab34faed9bf3e962d148d55f10dcd1dc`, unless an external dependency or historical revision is explicitly identified. Final-review citations refer to the corrected files in this change. Source excerpts may be de-indented or omit intervening lines; cited ranges identify the complete context. Quoted defects are preserved as evidence, not current usage guidance.

## G-001: Assertion-ledger scratch filename appends an extension in the docs but replaces it in code

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/memory.mdx:71-74`**

```text
and atomically replaces the file (`<state_path>.tmp`, then rename over
`state_path`)
```

Operators inspecting interrupted writes or configuring file handling will look for the wrong intermediate path.

**`meerkat-mobkit/src/runtime/memory.rs:124-137`**

```text
let tmp_path = self.state_path.with_extension("tmp");
```

with_extension replaces the existing extension. For the documented memory-ledger-state.json configuration, the intermediate file is memory-ledger-state.tmp, not memory-ledger-state.json.tmp.

### Independent adjudication

The literal scratch-path notation is wrong for the document's own .json example. This is not a request to change persistence or atomicity: PathBuf::with_extension replaces the extension, rather than appending .tmp. The surrounding ownership and rollback discussion can stay.

**`docs/concepts/memory.mdx:71-75`**

```text
and atomically replaces the file (`<state_path>.tmp`, then rename over
`state_path`)
```

This is current operator guidance, not an archived design target.

**`meerkat-mobkit/src/runtime/memory.rs:124-136`**

```text
let tmp_path = self.state_path.with_extension("tmp");
```

The following fs::write and fs::rename use this exact path. A memory-ledger-state.json state path therefore uses memory-ledger-state.tmp.

**Required correction:** Replace the appended-suffix notation with a scratch file obtained by replacing state_path's extension with .tmp (PathBuf::with_extension("tmp")); for memory-ledger-state.json, name memory-ledger-state.tmp. Preserve the rename, runtime-ownership, and rollback statements; change no code.

### Changes and final verification

**Changed:** `docs/concepts/memory.mdx`.

Replaced the appended .json.tmp notation with state_path.with_extension("tmp") and the memory-ledger-state.tmp example. Retained runtime snapshot ownership, rename, and rollback semantics.

**Validation:** Checked runtime/memory.rs persistence against the adjudicated source quote; the correction assertion for the replacement-extension path passed.

**Final review: pass.** The replacement now describes extension replacement, not suffix appending, and gives the correct scratch filename for the existing JSON example. The surrounding runtime-owned snapshot, rename, and failed-persist rollback qualifications remain intact.

**`docs/concepts/memory.mdx:73-78`**

```text
and atomically replaces the file (write to `state_path.with_extension("tmp")`,
then rename over `state_path`). The scratch path replaces the extension:
`memory-ledger-state.json` uses `memory-ledger-state.tmp`.
```

The final operator guidance fixes the exact originally misleading filename.

**`meerkat-mobkit/src/runtime/memory.rs:124-137`**

```text
let tmp_path = self.state_path.with_extension("tmp");
```

The actual persistence method writes this path and renames it over state_path; no persistence implementation was changed.

## G-002: Ledger-load documentation confuses canonical input requirements with normalization

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/memory.mdx:87-90`**

```text
- A well-formed hand-added entry is picked up only at the next restart, and
  only if its `entity`, `topic`, and `store` are canonical (the five supported
  store names) and its `fact` is non-empty; rows that fail that are dropped at
  load without a diagnostic.
```

The restart/recovery contract incorrectly says valid mixed-case or whitespace-padded entries are silently discarded, and the query discussion omits the corresponding normalized matching behavior.

**`meerkat-mobkit/src/runtime/bootstrap.rs:119-138`**

```text
let entity = MobkitRuntimeHandle::canonical_memory_token(&assertion.entity)?;
            let topic = MobkitRuntimeHandle::canonical_memory_token(&assertion.topic)?;
            let store = MobkitRuntimeHandle::canonical_memory_store(&assertion.store)?;
            let fact = assertion.fact.trim();
```

Loaded tokens are normalized before the replacement assertion is constructed; noncanonical case or surrounding whitespace is not itself grounds for dropping a row.

**`meerkat-mobkit/src/runtime/memory.rs:196-208`**

```text
let token = raw.trim().to_ascii_lowercase();
        if token.is_empty() { None } else { Some(token) }
```

Entity/topic accept any nonempty normalized token. Only store is restricted to the supported-store list after normalization; query filters undergo the same canonicalization.

### Independent adjudication

The loader actively canonicalizes each row; already-canonical spelling is not a precondition. Mixed ASCII case or surrounding whitespace alone cannot explain a dropped row. Narrow the proposed correction at the query boundary: the loader rejects unsupported stores, but memory_query uses and_then and treats an unnormalizable/unsupported optional filter as absent, not as a rejected request. Do not accidentally document new query validation.

**`meerkat-mobkit/src/runtime/bootstrap.rs:119-138`**

```text
let entity = MobkitRuntimeHandle::canonical_memory_token(&assertion.entity)?;
            let topic = MobkitRuntimeHandle::canonical_memory_token(&assertion.topic)?;
            let store = MobkitRuntimeHandle::canonical_memory_store(&assertion.store)?;
            let fact = assertion.fact.trim();
```

The reconstructed MemoryAssertion receives these normalized values. Empty trimmed facts or unsuccessful normalization are filtered out.

**`meerkat-mobkit/src/runtime/memory.rs:196-208`**

```text
let token = raw.trim().to_ascii_lowercase();
        if token.is_empty() { None } else { Some(token) }
```

Entity and topic only need to remain nonempty. Store additionally has to belong to MEMORY_SUPPORTED_STORES after the same normalization.

**`meerkat-mobkit/src/runtime/memory.rs:360-384`**

```text
let store = request
            .store
            .as_deref()
            .and_then(Self::canonical_memory_store);
```

Query normalization is shared, but its failure is represented as no filter. This prevents extending the load-time rejection claim to query-time behavior.

**Required correction:** In the restart/load bullet, say entity, topic, and store are trimmed and ASCII-lowercased; entity/topic must remain nonempty, store must normalize to one of the supported names, and fact must remain nonempty after trimming. In the query paragraph, qualify exact matches as matches on canonicalized tokens. Do not say that query filters must already be canonical or that unsupported query filters produce validation errors. Keep the advice to write via the runtime.

### Changes and final verification

**Changed:** `docs/concepts/memory.mdx`.

Documented trim/ASCII-lowercase normalization on load, nonempty entity/topic/fact requirements, and supported normalized stores. Qualified query exact matching as canonical-token matching and explicitly preserved the implementation's absent-filter treatment of empty/unsupported optional filters.

**Validation:** Read runtime/bootstrap.rs load filtering and runtime/memory.rs memory_query; source evidence and normalization/absent-filter prose assertions passed. No query validation behavior was added.

**Final review: pass.** Load-time normalization is accurately separated from query semantics. The page no longer requires already-canonical spelling and, importantly, does not invent query validation: invalid optional filters are documented as absent. Empty facts and unsupported normalized stores still fail the load filter.

**`docs/concepts/memory.mdx:89-95`**

```text
Loading trims and ASCII-lowercases `entity`, `topic`, and `store`;
```

The adjacent lines require nonempty entity/topic/fact and one of the five normalized supported store names, while explicitly allowing case/whitespace differences.

**`docs/concepts/memory.mdx:145`**

```text
Empty normalized filters and unsupported `store` filters are treated as absent, not as validation errors.
```

This preserves the adjudication's subtle distinction between loader rejection and optional-query-filter omission.

**`meerkat-mobkit/src/runtime/bootstrap.rs:119-138`**

```text
let entity = MobkitRuntimeHandle::canonical_memory_token(&assertion.entity)?;
```

The filter_map also canonicalizes topic/store, trims fact, rejects emptiness, and reconstructs a normalized assertion.

**`meerkat-mobkit/src/runtime/memory.rs:196-207`**

```text
let token = raw.trim().to_ascii_lowercase();
```

The shared canonicalizer trims and ASCII-lowercases; the store helper additionally checks MEMORY_SUPPORTED_STORES.

**`meerkat-mobkit/src/runtime/memory.rs:360-375`**

```text
.and_then(Self::canonical_memory_store);
```

A failed store normalization becomes None, and the later filter uses is_none_or. No new validation error is implied.

## G-003: conflict=true does not replace a fact assertion

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/memory.mdx:126`**

```text
Set `conflict: Some(true)` with `conflict_reason` to record a conflict signal instead of a normal assertion.
```

A reader changing only the flag in the preceding fact-bearing example inadvertently appends an assertion as well as the intended conflict-only signal.

**`meerkat-mobkit/src/runtime/memory.rs:301-326`**

```text
if let Some(fact) = fact {
```

Any supplied nonempty fact is appended as an assertion before conflict handling, regardless of the conflict flag.

**`meerkat-mobkit/src/runtime/memory.rs:319-350`**

```text
if conflict {
            let conflict_key = MemoryConflictKey {
```

Conflict creation is an independent branch, so a request containing both fact and conflict=true records both.

### Independent adjudication

A conflict-only request is supported, but setting the flag is not what makes it conflict-only. The fact and conflict branches are independent, and the preceding documentation example supplies a fact. The word 'instead' therefore materially misdescribes changing that example.

**`meerkat-mobkit/src/runtime/memory.rs:291-299`**

```text
let conflict = request.conflict.unwrap_or(false);
        if fact.is_none() && !conflict {
            return Err(MemoryIndexError::FactRequiredWhenConflictUnset);
        }
```

The implementation permits a missing fact specifically when conflict is true.

**`meerkat-mobkit/src/runtime/memory.rs:301-347`**

```text
if let Some(fact) = fact {
```

This branch appends an assertion before the separate if conflict branch inserts or replaces the keyed conflict signal. Supplying both yields both effects.

**Required correction:** Replace the 'instead of a normal assertion' sentence with: Set conflict: Some(true), optionally with conflict_reason, to record or update a conflict signal. For a conflict-only write set fact: None; a nonempty fact together with conflict: Some(true) records both an assertion and a conflict signal.

### Changes and final verification

**Changed:** `docs/concepts/memory.mdx`.

Explained that conflict=true records or updates a signal independently of an assertion; fact=None is required for a conflict-only write, and a nonempty fact plus conflict=true records both.

**Validation:** Read the independent fact and conflict branches in runtime/memory.rs; checked that the prose names both effects and the conflict-only fact=None condition.

**Final review: pass.** The guide explains both independent effects and explicitly requires fact=None for the conflict-only example. It correctly keeps conflict_reason optional and says a keyed conflict can be updated.

**`docs/concepts/memory.mdx:130`**

```text
For a conflict-only write, set `fact: None`; a non-empty `fact` together with `conflict: Some(true)` records both an assertion and a conflict signal.
```

The misleading 'instead' behavior is removed without changing the API.

**`meerkat-mobkit/src/runtime/memory.rs:291-337`**

```text
if let Some(fact) = fact {
```

memory_index appends the supplied fact in this branch, then independently executes if conflict and inserts the keyed signal. A missing fact is accepted when conflict is true.

## G-004: Current Memory panel API inventory omits six shipped read methods

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/memory.mdx:241-248`**

```text
It is strictly read-only and served by four `mobkit/memory/panel/*` RPC methods:
```

Embedders are told the available inspection surface is limited to the original four methods and miss durable dream detail and the shipped queues/ledgers.

**`meerkat-mobkit/src/http_console.rs:5604-5621`**

```text
"mobkit/memory/panel/records",
                    "mobkit/memory/panel/record",
                    "mobkit/memory/panel/dreams",
                    "mobkit/memory/panel/dream_runs",
                    "mobkit/memory/panel/audit_verdicts",
                    "mobkit/memory/panel/overview",
                    "mobkit/memory/panel/proposals",
                    "mobkit/memory/panel/injections",
                    "mobkit/memory/panel/harvests",
                    "mobkit/memory/panel/quarantine",
```

All ten are advertised when the panel store is present, with matching live dispatch arms at 6015-6046.

**`meerkat-mobkit/src/http_console.rs:2752-3013`**

```text
async fn handle_memory_panel_dream_runs(
```

The additional handlers serve durable dream detail, open usage-audit verdicts, per-scope overview/floors, pending proposals, injection history, and pending harvests, rather than hypothetical proposal-only endpoints.

### Independent adjudication

The current public concept page makes an exhaustive count claim ('four'), while six additional methods are both advertised and dispatched. This is a concrete inaccurate inventory, not a generic request for more documentation. The historical proposal's desired filters must not be copied into this current API table. Availability remains conditional on the panel store and applicable access grants.

**`meerkat-mobkit/src/http_console.rs:5604-5621`**

```text
"mobkit/memory/panel/dream_runs",
                    "mobkit/memory/panel/audit_verdicts",
                    "mobkit/memory/panel/overview",
                    "mobkit/memory/panel/proposals",
                    "mobkit/memory/panel/injections",
                    "mobkit/memory/panel/harvests",
```

The memory_panel.is_some() advertisement adds these six as well as the four already documented; subsequent capability filtering can restrict an individual caller's advertisement.

**`meerkat-mobkit/src/http_console.rs:6028-6046`**

```text
"mobkit/memory/panel/dream_runs" => {
            handle_memory_panel_dream_runs(memory_panel, &request.params, response_id).await
        }
```

The matching six dispatch arms are live implementation, not merely reserved method strings.

**`meerkat-mobkit/src/http_console.rs:2752-3013`**

```text
Some(json!({ "injections": injections, "realms": realms })),
```

The handlers provide durable dream-run detail, open audit verdicts, scope counts/bytes/floors, pending proposals, injection rows, and pending harvests. The overview takes realm selection; the other new lists additionally use the common limit. No identity/session/surface filters are parsed by the injections handler.

**Required correction:** Change the inventory to ten read-only panel methods. Add rows for dream_runs (durable run detail), audit_verdicts (open usage-audit verdicts), overview (per-scope status counts/body bytes and floors), proposals (pending proposal metadata and taint flag), injections (recent injection-ledger rows), and harvests (pending exit-interview harvests). Preserve provider/access-conditional availability and the original four rows. If parameters are named, document only those actually parsed; do not import proposed identity/session_key/surface filtering.

### Changes and final verification

**Changed:** `docs/concepts/memory.mdx`.

Expanded the read-only panel inventory from four to ten methods, adding dream_runs, audit_verdicts, overview, proposals, injections, and harvests with their implemented return semantics. Preserved provider/access-conditional availability and documented only realm/limit parameters actually parsed, explicitly excluding proposed injection filters.

**Validation:** Read http_console.rs handlers at the six added methods. A read-only Python inventory check confirmed exactly ten documented panel methods matching the current source method set.

**Final review: pass.** The final inventory has all ten live panel methods and accurately describes the six added result shapes. Provider/access-conditional availability remains stated. The final text explicitly avoids promoting proposed identity/session/surface injection filters into the shipped API.

**`docs/concepts/memory.mdx:260-277`**

```text
These six additional methods accept optional `realm` selection; all except
`overview` also accept `limit`. The current `injections` handler does not parse
identity, session-key, or surface filters proposed in the design notes.
```

The corrected table and parameter caveat match the actual handlers, not the historical UI wish list.

**`meerkat-mobkit/src/http_console.rs:5604-5621`**

```text
"mobkit/memory/panel/dream_runs",
                    "mobkit/memory/panel/audit_verdicts",
                    "mobkit/memory/panel/overview",
                    "mobkit/memory/panel/proposals",
                    "mobkit/memory/panel/injections",
                    "mobkit/memory/panel/harvests",
```

These methods join the original four only when the panel store exists; matching live dispatch arms are at 6030-6046.

**`meerkat-mobkit/src/http_console.rs:2752-3013`**

```text
"tainted": row.taint.is_some(),
```

Direct handler review confirmed durable dream detail, open audit verdicts, overview counts/bytes/floors/pressure, body-free proposals with propose-time taint, timestamp-sorted injections, and pending harvest metadata. An independent inventory comparison found exactly ten matching documented/source names.

## G-005: The architecture's unmarked Selector sections contradict its retired-stage as-built account

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/agent-memory-architecture.md:557-617`**

```text
### 8.3 Selector — recall judgment without a horizon

Replaces the lexical scorer (term-overlap, threshold 2) entirely.
```

Readers cannot distinguish the retained original design from supported runtime behavior and may expect nonexistent Selector calls, escalation, latency costs or CI coverage.

**`meerkat-mobkit/src/memory/coordinator.rs:7-13`**

```text
//! the §8.3 LLM Selector stage (P1.3) was retired unactivated, so nothing
//! here scores content beyond the wire-compat lexical recall the providers
//! already share.
```

This is not merely an unimplemented future stage: the implementation explicitly records its retirement without activation.

**`scripts/memory-evals:200-201`**

```text
RUNNABLE_STAGES = ("distiller", "steward", "hygienist")
```

There is no runnable Selector calibration stage, contradicting the architecture's as-built four-stage/mock-shuffle CI account at 1062-1067 as well as its unqualified on-path Selector claims.

**`meerkat-mobkit/src/identity_first/agent_memory.rs:1468-1491`**

```text
let score = record_relevance_score(&record, &terms);
                (score >= MIN_CONTEXTUAL_RELEVANCE_SCORE).then_some((score, record))
```

Lexical scoring is the live path; working-set/full-sweep LLM selection is not.

### Independent adjudication

Confirmed with substantial scope narrowing. The original Selector architecture is a genuine design target and is not false merely because it differs from today's implementation. However, this architecture-of-record already says the Selector is retired in §9, while its expressly 'As built' §11/§15 accounts still claim a fourth mock harness and selector-shuffle CI coverage. That current-status inconsistency is demonstrated. Resolve status/framing, not the design itself, and do not turn the audit into implementing or deleting a Selector.

**`docs/design/agent-memory-architecture.md:815-818`**

```text
ranking is the deterministic lexical provider path. The retired LLM Selector is
not part of composition.
```

The same document already establishes the relevant as-built boundary.

**`docs/design/agent-memory-architecture.md:1063-1067`**

```text
runs the bright-line ratchet and `memory-evals --check`, and four eval-harness
  integration tests drive every stage's mock lane end-to-end
```

Unlike the aspirational §8.3 design, this is explicitly asserted to be the as-built CI arrangement.

**`meerkat-mobkit/src/memory/coordinator.rs:7-13`**

```text
//! the §8.3 LLM Selector stage (P1.3) was retired unactivated, so nothing
//! here scores content beyond the wire-compat lexical recall the providers
//! already share.
```

Actual build/turn code calls recall_for_injection, and the bundled store delegates to select_recall_records; this is consistent with retirement, not an active LLM stage.

**`scripts/memory-evals:200-201, 1955-1975`**

```text
RUNNABLE_STAGES = ("distiller", "steward", "hygienist")
```

The live CLI choices were independently checked with --help. git ls-files 'meerkat-mobkit/tests/*eval_harness.rs' returns only distiller_eval_harness.rs, hygienist_eval_harness.rs, and steward_eval_harness.rs.

**Required correction:** Add a short explicit status note at the judgment-plane/Selector discussion: §8.3 and its invocation, cost, rollout, and comparison references preserve the original unactivated design; the Selector was retired and current recall uses the deterministic provider path. Mark dependent §14-§16 Selector references as original-plan context rather than present behavior, preferably via concise local cross-references rather than rewriting the target architecture. In the actual as-built §11 and §15 notes, replace four-stage/mock-shuffle claims with the three runnable Distiller/Steward/Hygienist harnesses; Selector artifacts are schema-checked historical calibration inputs only. Preserve the old design, survey, and dated calibration results.

### Changes and final verification

**Changed:** `docs/design/agent-memory-architecture.md`.

Added explicit original-plan/retired-unactivated Selector boundaries around the judgment-plane and Selector sections and the dependent comparison, rollout, and open-question sections. Corrected as-built CI notes to three runnable Distiller/Steward/Hygienist harnesses; Selector artifacts remain schema-checked historical calibration inputs, not live or mock-shuffle coverage. Removed a stale as-built full-sweep-cache reference from compaction reset prose.

**Validation:** Checked coordinator retirement comments, scripts/memory-evals runnable-stage choices, and the three eval-harness test files against adjudicated evidence. Historical Selector design and dated calibration results remain; all relevant status assertions passed.

**Final review: pass.** The original Selector design remains readable but is explicitly retired/unactivated at the judgment plane and Selector section. Comparison, rollout, cost, and open-question references are locally qualified. Both as-built CI statements now name three runnable harnesses and distinguish historical schema-checked Selector inputs. No Selector or prompt was restored or redesigned.

**`docs/design/agent-memory-architecture.md:492-497`**

```text
> The LLM Selector was retired unactivated; current recall uses the deterministic
> lexical provider path (§9). Its profiles/fixtures are historical calibration
> inputs, not a runnable stage.
```

The new boundary applies to dependent invocation, cost, rollout, and comparison references. Additional boundaries appear at 574-576, 1170-1172, 1223-1226, and 1311-1318.

**`docs/design/agent-memory-architecture.md:1089-1098`**

```text
and three eval-harness
  integration tests drive the runnable Distiller, Steward, and Hygienist mock
  lanes end-to-end
```

The fourth harness and Selector shuffle-stability CI claim are removed, with the same distinction repeated at 1281-1286.

**`meerkat-mobkit/src/memory/coordinator.rs:7-13`**

```text
//! the §8.3 LLM Selector stage (P1.3) was retired unactivated, so nothing
```

Both build and turn bodies actually call deterministic provider recall; the retirement is not inferred from a missing UI feature.

**`scripts/memory-evals:200-201`**

```text
RUNNABLE_STAGES = ("distiller", "steward", "hygienist")
```

CLI --help independently confirmed these exact choices. git ls-files returned only the corresponding three eval-harness integration tests, and CI's fmt-lint lane runs the schema and bright-line checks.

## G-006: The injection table still declares the shipped budgeted default echo-unsafe and fused

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/agent-memory-architecture.md:826-845`**

```text
Today mobkit *fuses* the injection into the user's own message text
```

The architecture misstates the principal safety property and default of the live identity-first path, while presenting opposite statements in adjacent sentences.

**`meerkat-mobkit/src/memory/coordinator.rs:661-668`**

```text
// Ask 1: deliver the recall as a separate injected-context body
        // (meerkat stamps ContentInput in `injected_context` as the typed
        // InjectedContext role → excluded from compaction indexing). The
        // user's message text is never touched.
        Ok(TurnInjection::Injected(vec![
            meerkat_core::ContentInput::Text(rendered.text),
        ]))
```

The current coordinator returns separate injection bodies, not a modified user message.

**`meerkat-mobkit/src/identity_first/agent_memory.rs:138-151`**

```text
per_turn_injection: AgentMemoryPerTurnInjection::Budgeted,
```

Budgeted is the current library default, as part of the same document already notes; the table's echo-safe column still says no/default-off.

**`meerkat-mobkit/src/memory/spawn_customizer.rs:43-53`**

```text
tracing::debug!(
```

The classic-mob paragraph also says the customizer warns at construction, but the supported build-only limitation is now reported at debug level because budgeted is the platform default.

### Independent adjudication

The table and adjacent 'Today ... fuses' assertion contradict both the current code and the document's own default-flipped parenthesis. The fix should describe typed delivery and the configured default, not promise ambient injection on every identity turn. The runtime can skip unsupported autonomous-host delivery, and classic mobs remain build-only. 'Echo-safe' here should be scoped to the typed message's exclusion from session semantic-memory indexing, not a new claim that no extraction path can ever reuse its content.

**`meerkat-mobkit/src/identity_first/agent_memory.rs:138-152`**

```text
per_turn_injection: AgentMemoryPerTurnInjection::Budgeted,
```

Budgeted, not Off, is the default AgentMemoryConfig posture.

**`meerkat-mobkit/src/memory/coordinator.rs:661-669`**

```text
Ok(TurnInjection::Injected(vec![
            meerkat_core::ContentInput::Text(rendered.text),
        ]))
```

The immutable content input remains separate from the returned injected bodies. The code explicitly names typed InjectedContext delivery and indexing exclusion.

**`meerkat-mobkit/src/identity_first/bridge.rs:1642-1654`**

```text
if !injected_context.is_empty() {
        spec = spec.with_injected_context(injected_context.to_vec());
    }
```

The bridge actually forwards the separate injected_context carrier rather than concatenating it with content.

**`meerkat-mobkit/src/memory/spawn_customizer.rs:43-53`**

```text
tracing::debug!(
```

The Budgeted constructor notice is debug-level, and its message explicitly limits ambient per-turn injection to identity-first members.

**`meerkat-mobkit/src/identity_first/runtime.rs:8520-8533`**

```text
return Ok((defanged, Vec::new()));
```

The preceding AutonomousHost delivery-mode guard declines ambient injection. A default setting is not evidence that all turns receive it.

**Required correction:** In §9.1 put separate typed InjectedContext bodies in the per-turn message-class column and mark indexing echo-safety yes; state budgeted is the current configured default on the supported identity-first delivery path. Convert fused/default-off/coupling prose into clearly historical context, preserving why both sides of the seam were necessary. Keep classic mobs build-only, remove 'off by default anyway', and change their constructor notice from warning to debug. Do not promise injection for Steer or unsupported runtime modes and do not alter runtime behavior.

### Changes and final verification

**Changed:** `docs/design/agent-memory-architecture.md`.

Made the current injection table describe separate typed InjectedContext bodies and their exclusion from session semantic-memory indexing. Described budgeted as the configured default on supported identity-first paths, with Steer and generic autonomous-host work exclusions. Reframed fused/default-off delivery as historical coupling context; retained classic-mob build/tool support and changed its construction notice to debug.

**Validation:** Read coordinator turn delivery, identity runtime delivery guards, and MemorySpawnCustomizer. Source quotes and typed-delivery/default/mode/debug caveat assertions passed; no universal extraction or every-turn injection guarantee was introduced.

**Final review: pass.** The current table now says separate typed context and scopes echo-safety to semantic-memory indexing. Budgeted is a configured default, not a promise that every turn injects. The Steer/generic autonomous-work exclusions and classic build-only path remain explicit; the construction notice is correctly debug-level. Fused/default-off behavior is preserved as history, not current implementation.

**`docs/design/agent-memory-architecture.md:850-858`**

```text
Separate typed `InjectedContext` bodies, never concatenated into user text
```

The following current-posture paragraph preserves supported-path and indexing-only qualifications.

**`meerkat-mobkit/src/memory/coordinator.rs:661-669`**

```text
Ok(TurnInjection::Injected(vec![
            meerkat_core::ContentInput::Text(rendered.text),
        ]))
```

The coordinator returns separate bodies. The bridge at identity_first/bridge.rs:1652-1653 forwards them with with_injected_context rather than concatenation.

**`meerkat-mobkit/src/identity_first/agent_memory.rs:138-152`**

```text
per_turn_injection: AgentMemoryPerTurnInjection::Budgeted,
```

The library's actual default agrees with the new table/posture.

**`meerkat-mobkit/src/identity_first/runtime.rs:8505-8530`**

```text
DeliveryPreparation::Work(meerkat_mob::MobRuntimeMode::AutonomousHost)
```

The earlier steer branch and this generic-work branch return no ambient bodies. The docs do not incorrectly exclude every console-human interaction on an autonomous host.

**`meerkat-mobkit/src/memory/spawn_customizer.rs:43-53`**

```text
tracing::debug!(
```

The construction notice confirms the classic path injects at build time only; final documentation at 877-883 matches it.

## G-007: Hygienist as-built note says revision-head support is missing after it was wired

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/agent-memory-architecture.md:802-805`**

```text
> Known gap (upstream-asks.md, ask 4 refinement): no service-level
> head-revision read exists, so rewrites send `expected_parent_revision:
> None` and §7.1 revision pinning stays `None` at capture time.
```

An explicitly as-built limitation falsely tells maintainers that the revision/CAS implementation is absent and all newly distilled provenance is unpinned.

**`meerkat-mobkit/src/memory/hygienist.rs:526-545`**

```text
.list_transcript_revisions(
                &session_id,
                meerkat_core::service::SessionTranscriptRevisionListQuery {
                    limit: Some(0),
                    offset: None,
                },
            )
```

The parked engine reads the head through the service, returns Some(head_revision) on success, falls back to None only for Unsupported, and forwards the captured value to rewrite_session_transcript at 574-579.

**`meerkat-mobkit/src/memory/distiller.rs:631-635`**

```text
let head_revision = session.transcript_revision().ok();
```

Ordinary transcript distillation captures a revision. build_record places it in EvidenceRef.revision at 1787-1792; compaction-discard evidence can still have no revision.

### Independent adjudication

The obsolete gap is in an explicitly as-built engine note, not just a future requirement. The parked Hygienist now asks for the head and passes it through its rewrite request; transcript distillation captures a revision too. This does not establish public activation, atomicity of the separate history/head reads, or universal resolution of every stored EvidenceRef. Those stronger interpretations are excluded.

**`meerkat-mobkit/src/memory/hygienist.rs:526-545`**

```text
Ok(list) => Some(list.head_revision),
            Err(meerkat_core::SessionError::Unsupported(_)) => None,
            Err(err) => return Err(err.to_string()),
```

The service-level list_transcript_revisions query uses limit Some(0). Only Unsupported falls back to no head in this path; other failures are not silently ignored.

**`meerkat-mobkit/src/memory/hygienist.rs:560-580`**

```text
expected_parent_revision,
```

The captured optional head is carried into SessionTranscriptRewriteRequest rather than always setting expected_parent_revision to None.

**`meerkat-mobkit/src/memory/distiller.rs:627-635, 1783-1794`**

```text
let head_revision = session.transcript_revision().ok();
```

SessionStoreTranscriptSource captures the revision, and build_record assigns it to EvidenceRef.revision. Capture failure and compaction-discard evidence can still be unpinned.

**Required correction:** Replace only the 'Known gap' paragraph with the implemented behavior: the internal engine reads a head through list_transcript_revisions and forwards it as expected_parent_revision; unsupported listing yields None, other listing errors fail the read. Transcript distillation captures session.transcript_revision() when available; failure and the compaction-discard path can retain None. Preserve the parked/not-publicly-activated statement and avoid claiming that all evidence readers resolve historical revisions or that history and head are acquired atomically.

### Changes and final verification

**Changed:** `docs/design/agent-memory-architecture.md`.

Replaced the obsolete missing-head-read gap with the internal Hygienist's list_transcript_revisions head capture and expected_parent_revision forwarding. Distinguished Unsupported=None from other read failures, separate history/head reads, transcript-distillation revision capture, and unpinned failed-capture/compaction-discard cases. Preserved parked/publicly-unactivated status.

**Validation:** Read the Hygienist read/rewrite implementation and verified Distiller revision-capture evidence. Assertions confirm no atomic-snapshot or universal historical-evidence-resolution claim.

**Final review: pass.** The obsolete no-head-read statement is replaced with implemented revision/CAS behavior. The correction accurately distinguishes Unsupported fallback from other read failures, does not promise atomic history/head acquisition, and preserves unpinned capture/discard cases and parked public activation.

**`docs/design/agent-memory-architecture.md:821-831`**

```text
> `expected_parent_revision` on rewrite. Unsupported revision listing yields
> `None`; other listing errors fail the read. History and head are separate
> reads, not an atomic snapshot.
```

The following lines qualify Distiller revision capture and explicitly say the public SDK/gateway do not activate the engine.

**`meerkat-mobkit/src/memory/hygienist.rs:530-545`**

```text
Ok(list) => Some(list.head_revision),
            Err(meerkat_core::SessionError::Unsupported(_)) => None,
            Err(err) => return Err(err.to_string()),
```

The real list_transcript_revisions call uses limit Some(0); rewrite forwards expected_parent_revision at line 578.

**`meerkat-mobkit/src/memory/distiller.rs:630-658`**

```text
let head_revision = session.transcript_revision().ok();
```

The optional captured value enters TranscriptSlice, and build_record assigns it to EvidenceRef.revision at 1783-1791. The discard path can retain None; this is capture, not universal resolution.

## G-008: P4 status incorrectly says no shipped host installs an operator resolver

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/agent-memory-architecture.md:1228-1234`**

```text
resolver SEAM only under §16 Q1 provisional keying — no shipped host
  installs a resolver, so operator recall composition is inert in stock
  deployments
```

A deployment enabling provisional scope is told its operator profile cannot enter composed build context even after authenticated console activity.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:12948-12961`**

```text
let resolver = Arc::new(meerkat_mobkit::ConsolePrincipalOperatorResolver::new());
            runtime.set_console_operator_resolver(resolver.clone());
```

The stock SDK gateway installs and shares this resolver when operator_scope is provisional, then attaches it to both the customizer and injector.

**`meerkat-mobkit/src/memory/coordinator.rs:180-201`**

```text
active.insert(identity.to_string(), principal.to_string());
```

Authenticated console interaction provides the per-identity principal binding; the resolver is not universally inert. The earlier §7.2 as-built note already describes these conditions correctly.

### Independent adjudication

The absolute assertion that no shipped host installs a resolver is directly contradicted by stock rpc_gateway. Its activation is nevertheless conditional, and G-009 limits what composition presently means: the shared operator metadata index, not generalized operator-body recall. Keep the provisional design question and resolver-less/library cases intact.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:12948-12961`**

```text
let resolver = Arc::new(meerkat_mobkit::ConsolePrincipalOperatorResolver::new());
            runtime.set_console_operator_resolver(resolver.clone());
```

The enclosing condition checks AgentMemoryOperatorScope::Provisional. This is shipped gateway setup, not a test or unused constructor.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:12996-13024`**

```text
.with_operator_resolver(agent_memory_operator_resolver.clone())
```

The same resolver is installed on build customization and the runtime injector.

**`meerkat-mobkit/src/http_console.rs:1075-1085`**

```text
resolver.note_interaction(request.identity.as_str(), subject);
```

The console ingress records the identity binding only when an authenticated access-view subject is available. The resolver starts without any binding.

**Required correction:** Align the §15 P4 status with the already-correct §7.2 activation note: stock rpc_gateway installs the provisional console-principal resolver when configured, and operator scope joins composed build metadata after a real authenticated console principal addresses that identity. Off, absent-resolver, and absent-principal cases remain inert. Retain the provisional keying caveat, independent steward proposal-routing behavior, and parked Hygienist status; do not imply shared bodies are automatically recalled.

### Changes and final verification

**Changed:** `docs/design/agent-memory-architecture.md`.

Aligned the P4 rollout status and scope note with the stock rpc_gateway's provisional ConsolePrincipalOperatorResolver. Operator metadata joins build composition after an authenticated principal addresses the identity; off, absent-resolver, and absent-principal cases stay inert. Preserved provisional keying, independently config-driven proposal routing, and parked Hygienist boundaries.

**Validation:** Read rpc_gateway resolver installation and verified the authenticated console-interaction source quote. Prose assertions confirm activation conditions and metadata-only composition rather than shared-body recall.

**Final review: pass.** P4 status now matches stock rpc_gateway installation and the authenticated interaction binding. It is carefully restricted to provisional metadata composition, not shared-body recall. Config-off, missing-resolver, and missing-principal cases remain inert; proposal routing and the parked Hygienist remain separately qualified.

**`docs/design/agent-memory-architecture.md:1267-1276`**

```text
Operator scope joins composed build-time metadata after an authenticated
  console principal addresses that identity; this does not enable shared-body
  recall.
```

The surrounding status names ConsolePrincipalOperatorResolver, provisional activation, and the three inert cases, consistent with the earlier scope note.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:12948-12961`**

```text
runtime.set_console_operator_resolver(resolver.clone());
```

The enclosing AgentMemoryOperatorScope::Provisional branch constructs the resolver; lines 13000 and 13023 share it with customizer and injector.

**`meerkat-mobkit/src/http_console.rs:1079-1085`**

```text
resolver.note_interaction(request.identity.as_str(), subject);
```

The call requires a real auth-context subject. The resolver's identity map starts empty and only gains entries through note_interaction.

## G-009: As-built scope account incorrectly includes shared bodies in ambient injection

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/agent-memory-architecture.md:552-556`**

```text
**As built:** the tool's explicit `recall` action (and the recall/manifest
RPCs) remain identity-scoped v1 surfaces; mob- and operator-scope content
reaches the agent through the composed build-time index and ambient injection,
not through explicit recall
```

Applications expect a newly promoted mob/operator record body to reach existing members on their next ordinary turn, which the bundled path does not do.

**`meerkat-mobkit/src/memory/coordinator.rs:607-624`**

```text
AgentMemoryRecallRequest {
                    identity: identity.clone(),
                    realm: self.config.realm.clone(),
```

Ambient injection calls the provider's v1 identity recall; it does not enumerate the composed scope set.

**`meerkat-mobkit/src/memory/sqlite_store.rs:844-855`**

```text
let scope = MemoryScope::Identity {
            realm: request.realm.clone(),
            identity: request.identity.as_str().to_string(),
        };
```

The bundled provider reads active records only from that identity scope.

**`meerkat-mobkit/src/memory/coordinator.rs:737-777`**

```text
"Memory index (metadata only; bodies are not loaded):\n{}"
```

Composed mob/operator/realm scope data enters the build-time metadata index. This is distinct from body recall; build-time selected bodies also use identity recall at 688-701.

### Independent adjudication

I checked whether the apparent identity request was widened inside the bundled provider; it is not. Build manifests compose scopes, but selected bodies for both build and turn paths go through the same identity-only recall. This is a false explicitly as-built claim, not grounds to abandon the aspiration that shared knowledge should reach members. The finding is bounded to the bundled/current path; a custom provider's possible behavior should not be prohibited by prose.

**`meerkat-mobkit/src/memory/coordinator.rs:604-625, 688-703`**

```text
AgentMemoryRecallRequest {
                    identity: identity.clone(),
                    realm: self.config.realm.clone(),
```

Both turn and build selected-body recall use this request rather than iterating the composed scope set.

**`meerkat-mobkit/src/memory/sqlite_store.rs:844-857`**

```text
let scope = MemoryScope::Identity {
            realm: request.realm.clone(),
            identity: request.identity.as_str().to_string(),
        };
```

recall_blocking queries active_scope_records only for the requested identity; it does not secretly merge mob, operator, or realm records.

**`meerkat-mobkit/src/memory/coordinator.rs:737-777`**

```text
"Memory index (metadata only; bodies are not loaded):\n{}"
```

render_scope_index really does enumerate composed scopes, but it explicitly emits only metadata.

**Required correction:** Rewrite the §8.2 as-built note to distinguish composed build-time metadata from body reads: configured/bound shared scopes contribute metadata to the build index; the current bundled explicit recall and automatic selected-body recall remain identity-scoped. Correct the adjacent §7.3 as-built phrase 'mob-scope reads compose through recall/manifest' to the same metadata-versus-body distinction so the document does not contradict itself. Keep wider shared-body recall as follow-up/design intent; do not implement it.

### Changes and final verification

**Changed:** `docs/design/agent-memory-architecture.md`.

Corrected both §7.3 and §8.2 as-built accounts: configured/bound shared scopes enter the composed build metadata index, while current bundled explicit recall and automatic selected-body recall remain identity-scoped. Wider shared-body reads remain follow-up/design intent, without constraining possible custom-provider behavior.

**Validation:** Read coordinator build/turn recall requests and metadata rendering; verified the bundled SQLite identity-scope lookup evidence. Metadata-versus-body correction assertions passed.

**Final review: pass.** Both conflicting as-built passages were corrected. Shared scopes contribute build metadata, whereas selected bodies and explicit v1 recall remain identity-scoped in the bundled implementation. The correction keeps wider recall as design intent and does not constrain custom providers.

**`docs/design/agent-memory-architecture.md:467-471`**

```text
shared scopes contribute metadata to the composed build-time index; the
current bundled explicit recall/manifest RPCs and automatic selected-body
recall remain identity-scoped (§8.2).
```

The adjacent provider/RPC as-built passage no longer contradicts the Recorder section.

**`docs/design/agent-memory-architecture.md:564-571`**

```text
The current bundled provider's selected-body recall is identity-scoped for
both build-time orientation and ambient turn injection, just as explicit
recall is.
```

This is followed by an explicit follow-up/custom-provider caveat, rather than an implemented ambient shared-body promise.

**`meerkat-mobkit/src/memory/coordinator.rs:737-777`**

```text
"Memory index (metadata only; bodies are not loaded):\n{}"
```

render_scope_index iterates the composed scope set; selected-body requests at 611-620 and 692-701 instead supply one identity/realm.

**`meerkat-mobkit/src/memory/sqlite_store.rs:844-855`**

```text
let scope = MemoryScope::Identity {
            realm: request.realm.clone(),
            identity: request.identity.as_str().to_string(),
        };
```

The provider does not secretly expand the request: it reads active_scope_records for this identity and then applies select_recall_records.

## G-010: Calibration running instructions describe an unreachable mock Selector instead of the supported stages

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`memory-evals/README.md:34-36`**

```text
`--mode mock` runs a trivial title-word-overlap selector purely to exercise the
scorecard plumbing — mock misses are expected and never fail the run.
```

Maintainers misinterpret what the advertised mock command exercises and which regressions CI actually rejects.

**`scripts/memory-evals:200-201`**

```text
RUNNABLE_STAGES = ("distiller", "steward", "hygienist")
```

argparse only permits these three stages; the README example invokes Distiller, not a word-overlap selector.

**`scripts/memory-evals:1495-1502`**

```text
print("  mock distiller extracts nothing (the doctrine's preferred output);")
```

The Distiller mock is a no-op extraction client. Steward and Hygienist run scripted replies through production parsing/validation.

**`scripts/memory-evals:1534-1539`**

```text
if hard_failures:
        print("  run FAILED — deterministic quarantine verdict mismatch.", file=sys.stderr)
        sys.exit(1)
```

Deterministic mismatches do fail mock runs; only judgment/extraction misses are informational.

### Independent adjudication

The command in the Running section selects Distiller, and the CLI does not accept Selector at all. The old overlap-scoring functions still exist in the script, which could superficially defend the text, but argparse makes their mock/live branches unreachable from the supported command line. Deterministic failures can fail mock runs; only judgment misses are informational. Preserve the explicitly historical Selector sections.

**`scripts/memory-evals:200-201, 1955-1975`**

```text
parser.add_argument("--stage", choices=RUNNABLE_STAGES, help="judgment stage to run")
```

RUNNABLE_STAGES is only distiller, steward, hygienist. Independently running scripts/memory-evals --help confirmed those exact choices.

**`scripts/memory-evals:1495-1502, 1534-1543`**

```text
print("  mock distiller extracts nothing (the doctrine's preferred output);")
```

The real mock lane is no-op extraction; hard_failures exit 1 while extraction misses only gate live mode. The end-to-end distiller_eval_harness test explicitly pins that distinction.

**`scripts/memory-evals:1698-1701, 1889-1893`**

```text
print("  scripted replies exercise the production parse → sanitize →")
```

Steward and Hygienist use scripted responses to exercise their production parsing/validation paths, not title-word-overlap selection.

**Required correction:** Replace the Running-section mock description with no-op Distiller extraction and scripted Steward/Hygienist production-parser/validator runs. State that deterministic failures gate every mode, while judgment/extraction misses are informational in mock mode. Change 'Live mode exists for every stage' to 'every runnable stage'. Leave retired Selector fixture formats and the historical run log as historical records, and do not restore the Selector CLI.

### Changes and final verification

**Changed:** `memory-evals/README.md`.

Replaced unreachable mock-Selector guidance with no-op Distiller extraction and scripted Steward/Hygienist parser/validator runs. Distinguished deterministic gating from informational mock judgment misses and limited live support to runnable stages. Also aligned the adjacent make memory-evals/CI command explanation with its build, corpus-check, and three-mock-lane recipe to avoid contradictory running guidance.

**Validation:** Checked scripts/memory-evals stage/mock branches and Makefile:110-128. scripts/memory-evals --check passed with four profiles and all fixture families; historical Selector fixture format and dated run log were preserved.

**Final review: pass.** Current running instructions now describe the three supported mock lanes and correctly distinguish deterministic gates from informational mock judgment misses. The adjacent make target correction is accurate. Historical Selector fixture/profile examples and the dated calibration log are preserved and clearly not supported commands.

**`memory-evals/README.md:40-45`**

```text
`--mode mock` uses no-op extraction for Distiller and scripted replies through
the production parsers and validators for Steward and Hygienist. Deterministic
failures gate every mode; judgment/extraction misses are informational in mock
mode.
```

The README no longer describes an unreachable title-overlap Selector or claims mock failures never gate.

**`scripts/memory-evals:1497-1500`**

```text
print("  mock distiller extracts nothing (the doctrine's preferred output);")
```

Steward at 1698-1699 and Hygienist at 1889-1891 explicitly exercise scripted production parsing/validation; their hard-failure branches exit 1.

**`Makefile:121-128`**

```text
	@scripts/memory-evals --stage distiller --mode mock
	@scripts/memory-evals --stage steward --mode mock
	@scripts/memory-evals --stage hygienist --mode mock
```

The target builds bins, runs --check, then these three lanes; ci depends on memory-evals. The new make/CI wording matches the actual recipe.

## G-011: The runner does not exit 3 when provider authentication is unavailable

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`memory-evals/README.md:12-14`**

```text
Live modes run real model calls where provider auth resolves
(exit-3 SKIP otherwise); they run in no CI lane yet (credentials).
```

Automation relying on wrapper exit 3 to detect skipped live calibration instead sees success and may incorrectly count a mock fallback as a real live pass.

**`scripts/memory-evals:1487-1494`**

```text
if code == EXIT_NO_AUTH:
            print("memory-evals: LIVE distiller SKIPPED — no resolvable provider auth")
            print("  (distiller-eval exit 3; running the --mock plumbing path instead;")
```

Exit 3 belongs to the child stage binary's auth probe. The runner consumes it, falls back to mock, and normally returns 0 if deterministic checks pass.

**`scripts/memory-evals:1687-1693`**

```text
print("  (steward-eval exit 3; running the scripted --mock path instead;")
```

Steward has the same fallback; Hygienist does too at 1878-1884. A controlled in-memory Distiller subprocess stub returning exit 3 verified that the runner emits MOCK-fallback and returns normally.

### Independent adjudication

The README's unqualified exit-3 SKIP conflates the child binary with the documented wrapper. All three wrapper stages consume an initial missing-auth probe exit 3 and switch to mock; success then means deterministic checks passed, not live calibration. I independently exercised this control flow for all three with in-memory subprocess/scorer stubs. The correction must retain failure cases: mock deterministic failures exit 1, and auth disappearing after an initially successful live probe is a configuration failure rather than the initial fallback.

**`meerkat-mobkit/src/bin/distiller_eval.rs:43-45, 259-265`**

```text
const EXIT_NO_AUTH: i32 = 3;
```

The child binary uses exit 3 on live-auth failure. steward_eval.rs and hygienist_eval.rs define the same exit code and use it for their auth errors.

**`scripts/memory-evals:1485-1502, 1509-1513, 1534-1543`**

```text
print("  (distiller-eval exit 3; running the --mock plumbing path instead;")
```

Distiller's wrapper explicitly chooses fallback, continues the fixture loop, and returns normally when its deterministic checks pass. A mid-run auth failure is handled separately.

**`scripts/memory-evals:1685-1695, 1738-1747, 1876-1886, 1930-1939`**

```text
print("  (steward-eval exit 3; running the scripted --mock path instead;")
```

Steward and Hygienist have the same distinction. The independent runpy control-flow probe observed initial child exit 3 consumed, subsequent calls made with mock=True, normal return on successful scores, and SystemExit(1) for controlled deterministic mismatches for each stage; no model or subprocess was invoked.

**Required correction:** Say that the stage binaries signal unavailable provider authentication with exit 3. For an initial live-auth probe returning 3, scripts/memory-evals prints LIVE ... SKIPPED, runs a MOCK-fallback scorecard, and normally exits 0 if deterministic checks pass. A zero wrapper exit alone is not evidence of live judgment coverage. Do not claim all later auth failures or deterministic failures are successful skips.

### Changes and final verification

**Changed:** `memory-evals/README.md`.

Separated stage-binary auth exit 3 from wrapper behavior: an initial missing-auth probe produces LIVE SKIPPED and a MOCK-fallback scorecard, normally exiting 0 only if deterministic checks pass. Explained that zero wrapper exit does not prove live coverage and that later auth loss is a failure, not a successful skip.

**Validation:** Verified the adjudicated child/wrapper source quotes and all three runner auth-fallback/mid-run-failure branches. Static correction assertions passed; no live model calls or new orchestration stubs were run during this fix wave.

**Final review: pass.** Child exit 3 is no longer attributed to the wrapper. The final text states initial-probe fallback, conditional zero exit, no evidence of live coverage from zero alone, deterministic failure, and mid-run authentication failure. I independently exercised all three stages' orchestration with in-memory stubs rather than trusting the earlier report.

**`memory-evals/README.md:13-19`**

```text
`scripts/memory-evals` prints `LIVE … SKIPPED`, runs a `MOCK-fallback`
scorecard, and normally exits 0 if its deterministic checks pass. A zero
wrapper exit alone does not certify live judgment coverage.
```

The surrounding lines identify stage-binary exit 3 and preserve both failure cases.

**`scripts/memory-evals:1485-1513`**

```text
if code == EXIT_NO_AUTH:
            print("memory-evals: LIVE distiller SKIPPED — no resolvable provider auth")
```

The wrapper consumes the initial code and later invokes fixtures with mock=not live; lines 1510-1513 reject mid-run auth loss.

**`scripts/memory-evals:1685-1747`**

```text
fail_config("provider auth disappeared mid-run; rerun the harness")
```

Steward has the same split, as does Hygienist at 1876-1939. Controlled probes for each stage returned 0 for initial-auth fallback with mock judgment misses, 1 for deterministic mismatches, and 2 for later auth loss. No model or subprocess was run.

## G-012: Bright-line exception documents search-only read access but the source enumerates and reclaims scopes

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`scripts/memory-bright-line-allow.txt:17-26`**

```text
# The §8.4 exception, pinned: the Distiller's post-compaction harvest reads
# meerkat's OWN session semantic store (MemoryStore::search over the discard
# range) READ-ONLY.
```

The enforcement ratchet's justification gives reviewers an incorrect description of why an existing dependency/source allowance is legitimate and which upstream-store operations it already permits.

**`meerkat-mobkit/src/memory/distiller.rs:789-806`**

```text
.enumerate_scoped(
                    &scope,
                    MemoryEnumerationRequest {
```

The harvest is exact paged enumeration, not one relevance-search query.

**`meerkat-mobkit/src/memory/distiller.rs:833-849`**

```text
.drop_scope(&MemoryOwner::canonical_session(session_id))
```

The same allowlisted source also uses the upstream store's lifecycle deletion API to reclaim permanently orphaned session scopes. Calling the entire exception read-only excludes actual sanctioned behavior.

### Independent adjudication

The scanner allowance is current enforcement guidance, and its justification inaccurately calls the whole source search-only/read-only. The actual sanctioned source performs exact paged enumeration and implements lifecycle deletion. Fix its documentary justification without changing the allowlist or runtime. Further narrow the preservation wording: the concrete delete path runs a bounded distillation attempt before cleanup; this audit should not invent a guarantee that cleanup waits for successful preservation in all outcomes or claim every rotation invokes cleanup.

**`meerkat-mobkit/src/memory/distiller.rs:789-801`**

```text
.enumerate_scoped(
                    &scope,
                    MemoryEnumerationRequest {
```

read_discards pages a session scope with a row ceiling. It is not a single relevance-search query.

**`meerkat-mobkit/src/memory/distiller.rs:827-846`**

```text
.drop_scope(&MemoryOwner::canonical_session(session_id))
```

The same HnswDiscardSource also mutates the upstream store through lifecycle reclamation, while not adding a retrieval index to MobKit's bundled store.

**`meerkat-mobkit/src/identity_first/runtime.rs:10732-10756`**

```text
.drop_orphaned_session_scope(
                        &session_key,
                        crate::memory::distiller::DistillCause::Delete,
                    )
```

Identity delete awaits distill_before_rotation, then calls cleanup. Its preceding comment explicitly states deletion proceeds at the pre-rotation timeout; do not strengthen this to guaranteed successful preservation.

**Required correction:** Update only the justification comments: the pinned exception allows bounded read-only enumerate_scoped harvesting of Meerkat's own session store plus the source's explicit drop_scope lifecycle reclamation of permanently abandoned session scopes (used after the delete pre-rotation distillation attempt). Replace obsolete 'MemoryStore::search', 'one query', and blanket 'read-only' descriptions, including the wiring allowance's description. Keep the exact dependency/file/pattern entries unchanged and reaffirm that MobKit's bundled store stays plain B-tree SQLite. Do not redesign preservation/cleanup semantics.

### Changes and final verification

**Changed:** `scripts/memory-bright-line-allow.txt`.

Corrected justification comments to bounded read-only enumerate_scoped harvest plus explicit drop_scope lifecycle reclamation of abandoned upstream session scopes after the delete pre-rotation distillation attempt. Removed search/one-query/blanket-read-only claims, including wiring comments. Kept the bundled store plain B-tree SQLite and made no successful-preservation guarantee.

**Validation:** A read-only Python comparison confirmed every non-comment allowlist entry is byte-for-byte equivalent to HEAD after whitespace stripping. scripts/check-memory-bright-line passed: dependencies clean and 18 memory-module source files clean.

**Final review: pass.** Only allowlist justification comments changed. They accurately describe bounded enumeration and explicit lifecycle scope reclamation without implying successful preservation before every cleanup. The exact five parsed non-comment allowances are unchanged, including file/pattern granularity; the bundled-store bright line is preserved.

**`scripts/memory-bright-line-allow.txt:17-24`**

```text
# bounded, read-only MemoryStore::enumerate_scoped over meerkat's OWN session
# semantic store. The same source supports explicit drop_scope lifecycle
# reclamation of permanently abandoned session scopes, used after the delete
# pre-rotation distillation attempt (not a guarantee of successful preservation).
```

Both the general justification and inline source/wiring comments now permit the actual sanctioned operations, not retrieval machinery in the bundled store.

**`meerkat-mobkit/src/memory/distiller.rs:778-845`**

```text
.drop_scope(&MemoryOwner::canonical_session(session_id))
```

read_discards pages enumerate_scoped with a bounded limit at 789-798, and the same source separately implements explicit drop_scope. These are not a single search query.

**`meerkat-mobkit/src/identity_first/runtime.rs:10731-10756`**

```text
// Bounded; deletion proceeds at the pre-rotation timeout.
```

Delete awaits a distillation attempt and then calls drop_orphaned_session_scope, so the new comments correctly avoid a guaranteed-success preservation claim.

**`scripts/check-memory-bright-line:94-112`**

```text
entry, sep, justification = line.partition("#")
```

Comparison using this actual parsing boundary proved all five entries identical to baseline. The unmodified scanner passed for dependencies and all 18 memory-module source files.

## G-013: Proposed session-budget proof uses the assembly cap instead of the session cap

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/memory-console-ui-proposal.md:257`**

```text
session `injected_bytes` ≤ 20KB, gauge monotone
```

Implementing the proposed proof literally labels permitted 20-60 KiB sessions as budget violations and displays the wrong health gauge.

**`meerkat-mobkit/src/memory/coordinator.rs:46-47`**

```text
pub(crate) const MAX_INJECTED_ASSEMBLY_BYTES: usize = 20 * 1024;
pub(crate) const MAX_INJECTED_SESSION_BYTES: usize = 60 * 1024;
```

The proposal cites the real assembly constant but applies it to cumulative session accounting.

**`meerkat-mobkit/src/memory/coordinator.rs:594-599`**

```text
MAX_INJECTED_ASSEMBLY_BYTES
                        .min(MAX_INJECTED_SESSION_BYTES.saturating_sub(used))
```

A session may legitimately exceed 20 KiB across multiple bounded injections and reach 60 KiB before compaction/reset.

### Independent adjudication

This is not modernization of a dated proposal: I inspected its named caf18995 snapshot, which already had 20*1024 assembly and 60*1024 session constants. The proposal cites the assembly constant while testing the cumulative session field, so its feasibility premise was wrong at that snapshot too. Narrow 'session cap' to the coordinator's tracked per-turn counter, not all build plus turn bytes over a durable session lifetime.

**`meerkat-mobkit/src/memory/coordinator.rs:46-47`**

```text
pub(crate) const MAX_INJECTED_ASSEMBLY_BYTES: usize = 20 * 1024;
pub(crate) const MAX_INJECTED_SESSION_BYTES: usize = 60 * 1024;
```

Independently checked both the current file and git show caf18995:meerkat-mobkit/src/memory/coordinator.rs; these constants already had the same values in the proposal's named snapshot.

**`meerkat-mobkit/src/memory/coordinator.rs:588-599, 641-653`**

```text
MAX_INJECTED_ASSEMBLY_BYTES
                        .min(MAX_INJECTED_SESSION_BYTES.saturating_sub(used)),
```

Each turn is assembly-bounded, while rendered bytes accumulate against the separate session budget. The map can also be cleared at its tracked-session ceiling.

**`meerkat-mobkit/src/memory/coordinator.rs:440-450, 718-733`**

```text
.remove(session_key);
```

Compaction resets the counter; build assembly has its own 20 KiB budget and records a build injection without adding to this session-keyed per-turn counter.

**Required correction:** Keep the proposed gauges but correct their premises in the feasibility note, §3.3 mockup/data source, §3.6, and §5: 20 KiB is the per-assembly limit; the session-keyed per-turn injected_bytes counter is bounded by 60 KiB between state resets. The counter resets on compaction and can be lost on coordinator/process reset or bounded-map clearing; it is not globally monotone or a durable lifetime total. Use 'monotone within one uninterrupted accounting interval' only if needed. Do not implement the proposed health endpoint or change any budget.

### Changes and final verification

**Changed:** `docs/design/memory-console-ui-proposal.md`.

Corrected feasibility, Knowledge Lens mockup/data source, Health, and verification predicates to 20 KiB per assembly versus 60 KiB in the session-keyed per-turn counter. Excluded build assemblies from that counter and documented compaction, coordinator/process reset, and bounded-map clearing; monotonicity is only within one uninterrupted accounting interval, not a durable lifetime.

**Validation:** Read coordinator constants, turn accounting, reset, bounded-map clearing, and separate build assembly. Budget/reset/interval assertions passed; no proposed health endpoint or budget implementation was added.

**Final review: pass.** Every corrected gauge/predicate now separates 20 KiB assembly from 60 KiB tracked turn accounting and explicitly excludes builds from the latter. Compaction, process/coordinator recreation, and bounded-map clearing qualify monotonicity and lifetime. These were already true at caf18995; no future health endpoint or budget implementation is claimed.

**`docs/design/memory-console-ui-proposal.md:157`**

```text
Build assembly has its own 20 KiB cap and does not add to this per-turn counter; the gauge is not a durable lifetime total.
```

The feasibility note, mockup, health source, and verification table repeat the same corrected 20/60 KiB and accounting-interval distinction.

**`meerkat-mobkit/src/memory/coordinator.rs:46-47`**

```text
pub(crate) const MAX_INJECTED_ASSEMBLY_BYTES: usize = 20 * 1024;
pub(crate) const MAX_INJECTED_SESSION_BYTES: usize = 60 * 1024;
```

The same constants exist at the proposal's historical snapshot, so this is a false-premise correction rather than modernization of the proposal.

**`meerkat-mobkit/src/memory/coordinator.rs:589-599`**

```text
MAX_INJECTED_ASSEMBLY_BYTES
                        .min(MAX_INJECTED_SESSION_BYTES.saturating_sub(used)),
```

Turn assembly subtracts tracked bytes; lines 641-650 update/reset the map, while build assembly at 718-733 uses its own assembly budget and no session counter.

## G-014: Session-key-only injection duplicates are not proof of a dedup violation

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/memory-console-ui-proposal.md:257`**

```text
group *turn* ledger rows by `(record_id, identity, session_key)` — any count > 1 renders a red **DUP** badge
```

The proposal's standing regression proof would produce false red violations during normal long-lived sessions and after process-local dedup state is lost.

**`meerkat-mobkit/src/memory/coordinator.rs:440-450`**

```text
pub fn on_session_compacted(&self, session_key: &str) {
        self.session_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(session_key);
    }
```

Compaction deliberately clears the session's dedup set without changing the session key, so the same record can correctly be injected again.

**`meerkat-mobkit/src/memory/records.rs:316-329`**

```text
pub session_key: Option<String>,
    pub surface: InjectionSurface,
    pub at_ms: u64,
```

Ledger rows have a session key and timestamp but no compaction/reset epoch, so the proposed grouping cannot distinguish valid post-compaction re-injection from a duplicate within one dedup interval.

### Independent adjudication

The proposed grouping is insufficient to prove a violation. A second row with the same record/identity/session can legitimately follow compaction, process recreation, or tracked-map clearing. I checked caf18995 as well: on_session_compacted already removed the session-state entry then. The proposed diagnostics can remain, but labeling every repeated group a conclusive red dedup failure is a false invariant, not merely an unimplemented UI ambition.

**`meerkat-mobkit/src/memory/coordinator.rs:440-450`**

```text
pub fn on_session_compacted(&self, session_key: &str) {
        self.session_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(session_key);
```

Resetting dedup does not change the session key. The same removal is present at caf18995:380-384.

**`meerkat-mobkit/src/memory/coordinator.rs:641-651`**

```text
if !guard.contains_key(key) && guard.len() >= MAX_TRACKED_INJECTION_SESSIONS {
                guard.clear();
            }
```

Compaction is not the only possible boundary of an uninterrupted dedup interval.

**`meerkat-mobkit/src/memory/records.rs:316-329`**

```text
pub session_key: Option<String>,
    pub surface: InjectionSurface,
    pub at_ms: u64,
```

The ledger has no compaction/coordinator-reset epoch. The other fields are only record_id and identity; a key-only group cannot establish the reset boundary.

**Required correction:** In §3.3, §5 and the dependent walkthrough/phase-2 claims, retain repeat-row inspection as a diagnostic, but remove 'any count > 1' as a conclusive violation proof. A dedup violation requires evidence that both injections occurred in the same uninterrupted dedup interval. With only the existing ledger fields, label that verdict UNVERIFIABLE (or possible repeat) and name missing reset-boundary evidence. Preserve distinct build-overlap diagnostics, which are not the same invariant. Do not add ledger epochs, change dedup, or modify the UI implementation in this documentation task.

### Changes and final verification

**Changed:** `docs/design/memory-console-ui-proposal.md`.

Recast repeated session-key groups as possible-repeat diagnostics, not automatic red DUP proofs. Updated Knowledge Lens, verification widgets, walkthrough, phase-2 claims, and the dependent competition summary. A violation requires same uninterrupted dedup-interval evidence; current rows lack reset epochs, so the proof remains UNVERIFIABLE. Kept build-overlap diagnostics distinct.

**Validation:** Read coordinator compaction/reset and bounded-map clearing and verified InjectionLogEntry fields. Assertions confirm missing reset-boundary evidence, separate build overlap, and no automatic HOLDING transition merely because panel/injections exists.

**Final review: pass.** The proposal no longer treats repeated session-key groups as conclusive dedup failures. Knowledge Lens, walkthrough, verification table, phase 2, and the dependent competition summary preserve the distinction between diagnostic repeats, provable same-interval duplicates, and build overlap. A panel/injections endpoint alone no longer turns the proof green.

**`docs/design/memory-console-ui-proposal.md:155`**

```text
A violation requires both injections to belong to the same uninterrupted dedup interval. Existing rows have no reset epoch, so that verdict is **UNVERIFIABLE — missing reset-boundary evidence**.
```

The explicit missing-evidence requirement is carried through the ECHO-SAFETY table and phase-2 rollout.

**`docs/design/memory-console-ui-proposal.md:312`**

```text
ECHO-SAFETY does **not** automatically flip to HOLDING: reset-boundary evidence is still needed to prove turn dedup
```

The formerly contradictory dependent phase claim is also corrected.

**`meerkat-mobkit/src/memory/coordinator.rs:445-450`**

```text
.remove(session_key);
```

Compaction deliberately resets the same session key's state; bounded-map clearing at 645-646 is another legitimate reset. Both behaviors were independently found at caf18995.

**`meerkat-mobkit/src/memory/records.rs:322-329`**

```text
pub session_key: Option<String>,
    pub surface: InjectionSurface,
    pub at_ms: u64,
```

The ledger has no reset epoch. Record/identity/session/timestamp cannot distinguish valid reinjection across resets from a duplicate in one interval.

## G-015: Proposed trust-audit predicates reject valid reviewed memory

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/memory-console-ui-proposal.md:258-259`**

```text
for every record with `derived_from[]`, assert trust ≤ min(parent trust)
```

Valid steward verification and reviewed quarantine release are reported as trust-lattice violations, undermining the very inspection guarantee the proposal describes.

**`meerkat-mobkit/src/memory/staged.rs:608-623`**

```text
if *trust == TrustTier::AgentVerified {
```

A steward may retier a record with a verification claim to agent_verified, subject to evidence checks and the transitive taint ceiling. This contradicts the next table row's universal 'no record with provenance.author in agent/distiller/steward above agent_observed' predicate.

**`meerkat-mobkit/src/memory/sqlite_store.rs:3104-3110`**

```text
"UPDATE records SET trust = ?1, updated_at_ms = ?2 WHERE memory_id = ?3"
```

A valid retier does not replace the original provenance author, so an agent-authored record may legitimately be agent_verified.

**`meerkat-mobkit/src/memory/steward.rs:3073-3094`**

```text
trust: TrustTier::AgentObserved,
                            derived_from: vec![record.id.clone()],
```

Reviewed quarantine release intentionally creates an agent_observed copy derived from its untrusted origin. The ceiling is agent_observed for any transitive taint, not the minimum parent trust.

**`console/src/panels/MemoryPanel.tsx:644-654`**

```text
rank > TRUST_RANK.agent_observed
```

The current UI copied the overly broad LLM-origin predicate. Correcting the documentation does not fix that UI behavior; any runtime/UI repair is a separately scoped follow-up, not part of this document audit.

### Independent adjudication

Both proposed predicates misstate existing validator law rather than merely proposing a future visualization. A reviewed agent_verified retier preserves the original agent authorship, and quarantine release deliberately creates an agent_observed derivative of an untrusted origin. Both were already supported in caf18995, so the dated proposal framing does not rescue these predicates. A similar LLM-origin test exists in the current UI; that code issue is explicitly separate and must not be silently repaired in a documentation-only wave.

**`meerkat-mobkit/src/memory/staged.rs:608-624, 718-725`**

```text
let steward_retier = is_retier && matches!(batch.author, MemoryAuthor::Steward { .. });
```

The validator distinguishes initial LLM writes from steward retiering. AgentVerified requires a steward and verification claim, with the transitive taint ceiling still enforced; caf18995 had the same exception.

**`meerkat-mobkit/src/memory/sqlite_store.rs:3103-3111`**

```text
"UPDATE records SET trust = ?1, updated_at_ms = ?2 WHERE memory_id = ?3"
```

Retiering does not replace provenance.author, so inspecting original authorship and current tier alone falsely condemns valid reviewed records.

**`meerkat-mobkit/src/memory/steward.rs:6047-6103`**

```text
assert_eq!(upgraded.trust, TrustTier::AgentVerified);
```

The existing verified_retier_requires_resolvable_evidence test seeds an agent-authored record and demonstrates successful steward retiering once cited evidence resolves. This test was read, not executed.

**`meerkat-mobkit/src/memory/steward.rs:3073-3094`**

```text
trust: TrustTier::AgentObserved,
                            derived_from: vec![record.id.clone()],
```

Reviewed quarantine release copies the origin and tombstones it. The same release behavior appears at caf18995:2605-2619, directly disproving a universal min(parent trust) rule.

**`meerkat-mobkit/src/memory/staged.rs:733-766`**

```text
if record.trust == TrustTier::Untrusted
            || record.ever_quarantined
            || matches!(record.status, RecordStatus::Quarantined { .. })
```

The implementation walks both supersedes and derived_from ancestry; any such taint invokes an agent_observed ceiling, not the minimum tier of arbitrary parents.

**`console/src/panels/MemoryPanel.tsx:644-654`**

```text
rank > TRUST_RANK.agent_observed
```

latticeInvariants currently combines this with agent/distiller/steward provenance. Record as a separate UI follow-up; a docs edit alone will not correct its false positives.

**Required correction:** Correct the §5 TAINT WALL and LATTICE predicates and their dependent phase-1 claims: initial LLM-created writes are capped at agent_observed, but an authorized reviewed agent_verified retier with valid verification is allowed; transitive untrusted/quarantined/ever-quarantined ancestry through supersedes or derived_from imposes an agent_observed ceiling, not min(parent trust). A display without the needed review, verification, or ancestry evidence cannot prove a violation solely from author plus tier and must say UNVERIFIABLE. Preserve the verification-widget goal; do not redesign the lattice or edit MemoryPanel.tsx. Note the existing UI predicate as a separately scoped follow-up.

### Changes and final verification

**Changed:** `docs/design/memory-console-ui-proposal.md`.

Corrected TAINT WALL/LATTICE and dependent phase claims: initial LLM writes are capped at agent_observed; authorized steward-reviewed agent_verified retiers with valid verification are allowed without changing original author. Untrusted/quarantined/ever-quarantined self or transitive supersedes/derived_from ancestry imposes agent_observed, not minimum-parent trust. Missing review, verification, marker, or ancestry evidence means UNVERIFIABLE. Added an explicit out-of-scope existing MemoryPanel.tsx predicate follow-up.

**Validation:** Read staged.rs tier/taint validation, Steward reviewed release, and the existing MemoryPanel.tsx author/tier predicate. Corrected-law assertions passed. Source-diff checks confirm MemoryPanel.tsx and all runtime/UI code remain unchanged.

**Final review: pass.** The documentary law is now correct: initial LLM writes have the ceiling, authorized reviewed agent_verified retiering is allowed, and tainted self/transitive ancestry imposes agent_observed rather than min(parent trust). Missing verification/history/ancestry means UNVERIFIABLE. The current UI predicate's known false positive is explicitly called out as separate; its intentional non-fix is not counted as a failed documentation correction or a regression.

**`docs/design/memory-console-ui-proposal.md:264`**

```text
Original `provenance.author` remains unchanged by retiering, so **author plus current tier alone cannot prove a violation**; missing review/verification history means **UNVERIFIABLE**.
```

The same table names the steward review exception and checks both supersedes and derived_from ancestry. Phase 1 at line 306 preserves these evidence requirements.

**`docs/design/memory-console-ui-proposal.md:269-274`**

```text
these design assertions does **not** fix that implementation; a separately
scoped code change is required. No UI/runtime change is implied here.
```

This explicitly avoids claiming the runtime/UI predicate has been repaired.

**`meerkat-mobkit/src/memory/staged.rs:607-628`**

```text
if !steward || !existing.has_verification {
```

AgentVerified retiering requires steward authorship and a verification claim, then checks transitive taint. check_tier_assignment at 718-725 distinguishes retiering from initial LLM writes, and chain_reaches_taint at 733-766 walks both lineage edge types.

**`meerkat-mobkit/src/memory/sqlite_store.rs:3105-3111`**

```text
"UPDATE records SET trust = ?1, updated_at_ms = ?2 WHERE memory_id = ?3"
```

The retier leaves original provenance untouched. The existing verified_retier_requires_resolvable_evidence test at steward.rs:6047-6103 demonstrates agent-authored records can legitimately become AgentVerified.

**`meerkat-mobkit/src/memory/steward.rs:3077-3092`**

```text
trust: TrustTier::AgentObserved,
                            derived_from: vec![record.id.clone()],
```

Reviewed quarantine release can create this derivative of an untrusted origin, disproving the former minimum-parent-tier rule. The same behavior existed at caf18995.

**`console/src/panels/MemoryPanel.tsx:648-653`**

```text
rank > TRUST_RANK.agent_observed
```

The existing author-plus-tier predicate remains deliberately present. An exact byte comparison against baseline proved this file unchanged.

## G-016: Timeline querying cannot supply the proposal's exact revision-pinned evidence read

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/memory-console-ui-proposal.md:119`**

```text
Evidence is revision-pinned, so hygienist rewrites don't break it.
```

The proposal's 'zero backend change' feasibility claim promises an exact birth/evidence window robust to transcript rewrites that the named endpoint cannot provide.

**`meerkat-mobkit/src/console_aggregator/types.rs:214-232`**

```text
pub struct ConsoleTimelineWindowQuery {
```

The timeline query accepts identity, conversation_id, before/after cursors, mode and limit; it has no transcript revision, generation or source-message range selector.

**`meerkat-mobkit/src/http_console.rs:6142-6165`**

```text
aggregator.query_timeline_windowed(query.clone())
```

The RPC reads projected console frames, not a pinned transcript revision. A provenance reference carrying a revision does not make this endpoint revision-aware.

**`console/src/ConsoleApp.tsx:2485-2504`**

```text
(frame) => frame.sessionId === evidence.session_id,
```

The shipped click-through fetches a recent timeline window and filters by session ID only. It does not consume evidence.revision, and the detail UI explicitly labels the excerpt approximate.

### Independent adjudication

Exact revision-pinned provenance is a legitimate design goal. The false part is specifically claiming that existing query_timeline supplies it with zero backend change. That endpoint accepts neither revision nor transcript source range/generation and serves projected frames. Its caf18995 request shape had the same limitation. A record containing a revision reference does not turn the named endpoint into a revision resolver.

**`meerkat-mobkit/src/console_aggregator/types.rs:214-232`**

```text
pub struct ConsoleTimelineWindowQuery {
```

Its complete selector set is identity, conversation_id, after, before, mode and limit. The independently inspected caf18995 version also lacks revision, generation and source-message range.

**`meerkat-mobkit/src/http_console.rs:6142-6171`**

```text
aggregator.query_timeline_windowed(query.clone())
```

The documented RPC dispatches to projected timeline querying, not to a historical transcript-revision reader.

**`console/src/ConsoleApp.tsx:2484-2508`**

```text
(frame) => frame.sessionId === evidence.session_id,
```

The shipped click-through asks for recent frames and filters by session, without resolving evidence.revision.

**`console/src/panels/MemoryPanel.tsx:1225-1228, 1582-1584`**

```text
approximate against the console timeline.
```

The UI already acknowledges approximation; the document should not assert stronger resolution than its stated data source supplies.

**Required correction:** Keep the proposed evidence link and exact-evidence aspiration. In §3.2, the §4 'exact turn' walkthrough, and dependent phase-1 wording, describe the existing query_timeline click-through as a best-effort projected excerpt, possibly approximate or unavailable, not an exact revision-pinned transcript window. State that exact historical evidence resolution requires a revision-aware transcript read seam outside this client-only path. Do not implement that seam, remove revision references, or claim revision pinning itself is undesirable.

### Changes and final verification

**Changed:** `docs/design/memory-console-ui-proposal.md`.

Reframed existing query_timeline click-through as a best-effort projected excerpt, approximate or unavailable, in Record Biography, Pipeline, the exact-turn walkthrough, the dependent re-entry investigation, and phase 1. Preserved exact revision-pinned evidence as an aspiration requiring a separate revision-aware transcript read seam.

**Validation:** Read ConsoleTimelineWindowQuery selectors and checked actual query dispatch and UI evidence-resolution source quotes. Approximation/unavailable/read-seam assertions passed; the new current-memory-guide link and heading target resolve.

**Final review: pass.** The named query_timeline path is now honestly best-effort projected evidence, not a revision-aware transcript resolver. Biography, Pipeline, walkthrough, phase 1, and dependent re-entry checks all preserve approximation/unavailability. Exact revision-pinned evidence remains a legitimate future seam, not an abandoned goal or a falsely shipped feature.

**`docs/design/memory-console-ui-proposal.md:124`**

```text
the RPC accepts no transcript revision, generation, or source-message-range selector. A captured revision reference does not make this endpoint revision-aware.
```

The rest of the paragraph retains exact historical resolution as work requiring a separate read seam; phase 1 at line 301 repeats that boundary.

**`meerkat-mobkit/src/console_aggregator/types.rs:214-232`**

```text
pub struct ConsoleTimelineWindowQuery {
```

The complete struct contains identity, conversation_id, after, before, mode, and limit only. The historical caf18995 struct has the same relevant limitation.

**`meerkat-mobkit/src/http_console.rs:6142-6168`**

```text
aggregator.query_timeline_windowed(query.clone())
```

The RPC reads projected timeline frames, not historical transcript revisions.

**`console/src/ConsoleApp.tsx:2484-2507`**

```text
(frame) => frame.sessionId === evidence.session_id,
```

The actual click-through uses recent frames and session filtering, not evidence.revision. MemoryPanel.tsx:1582-1589 already labels approximation, consistent with the corrected documentary promise.

## G-017: The follow-a-fact walkthrough sends ordinary Distiller writes through the proposal queue

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/design/memory-console-ui-proposal.md:242`**

```text
Either way, the proposal appears in **Pipeline** (§3.4) with `taint@propose: clean`, and the new row lands in **Records** on the next live refresh
```

Operators following the proposed walkthrough look for a nonexistent pending proposal/taint-at-propose row and mistake a successful direct extraction for a missing pipeline transition.

**`meerkat-mobkit/src/memory/distiller.rs:1699-1709`**

```text
ProposedAction::Remember => {
                    self.provider
                        .remember_authored(&scope, record, author.clone())
```

Validated extraction output writes directly through remember_authored or supersede_authored into the identity store. It does not call propose or create a pending mob/operator proposal.

**`meerkat-mobkit/src/http_console.rs:2908-2922`**

```text
match store.pending_proposals(realm, limit).await {
```

The Pipeline proposal data source enumerates the separate pending-proposal queue, so an ordinary successful Distiller record cannot appear there merely because extraction occurred.

### Independent adjudication

I distinguished the Distiller's in-process ProposedAction parser from the durable mob/operator proposal queue. Despite that internal name, successful ordinary extraction writes directly to identity records. The named historical snapshot did this too. The walkthrough therefore conflates separate operations; fixing the example must not add proposal creation to runtime extraction or promise every model pass emits a record.

**`meerkat-mobkit/src/memory/distiller.rs:1698-1725`**

```text
ProposedAction::Remember => {
                    self.provider
                        .remember_authored(&scope, record, author.clone())
                        .await
                }
```

The alternative Update calls supersede_authored. Receipt status counts quarantined records. Neither branch enqueues a durable proposal; caf18995 likewise called these two methods at 1510/1515.

**`meerkat-mobkit/src/http_console.rs:2908-2922`**

```text
match store.pending_proposals(realm, limit).await {
```

The Pipeline's proposal source is a separate queue; a direct Distiller record does not appear in it by virtue of being extracted.

**`docs/design/memory-console-ui-proposal.md:242, 310`**

```text
Either way, the proposal appears in **Pipeline** (§3.4) with `taint@propose: clean`
```

The walkthrough promises that queue transition for the ordinary extraction step, and the later event-payload question repeats the same assumption about proposal ids.

**Required correction:** In §4 step 2, say that when extraction produces successful ordinary writes they appear as identity-scope records with Distiller provenance; gated writes appear as quarantined records in the reviewer-visible quarantine surface. A pending mob/operator proposal is a distinct action, not an automatic extraction hop, and no-op extraction creates neither. Remove the implied taint@propose row for this step. Align the open event-payload question with record ids for direct extraction, while preserving proposal ids for actual proposal actions. Keep the proposed completion event/UI flow as a proposal; do not add runtime writes or events.

### Changes and final verification

**Changed:** `docs/design/memory-console-ui-proposal.md`.

Corrected the follow-a-fact extraction hop to direct identity-scope records with Distiller provenance, or reviewer-visible quarantined records when gated. No-op extraction creates neither records nor proposals; mob/operator proposals are separate actions. The proposed completion-event payload question now asks about direct record ids rather than automatic proposal ids.

**Validation:** Read Distiller remember_authored/supersede_authored branches and checked the separate pending_proposals handler. Direct-write/no-op/separate-proposal assertions passed; completion events and UI flow remain proposed rather than implemented by this task.

**Final review: pass.** The walkthrough now distinguishes ordinary direct identity-store writes, gated/quarantined writes, no-op extraction, and separately enqueued mob/operator proposals. The event-payload question now asks for direct record IDs rather than implying extraction writes proposal IDs. The completion event remains expressly proposed.

**`docs/design/memory-console-ui-proposal.md:247`**

```text
No-op extraction creates neither a record nor a proposal. A pending mob/operator promotion proposal is a distinct action, not an automatic extraction hop
```

The preceding sentence describes successful direct identity-scope Records with Distiller provenance and reviewer-visible quarantine, not a mandatory Pipeline hop.

**`docs/design/memory-console-ui-proposal.md:322`**

```text
Proposal ids belong to actual mob/operator proposal actions, not ordinary direct extraction.
```

The dependent event-payload question is consistent with the corrected walkthrough.

**`meerkat-mobkit/src/memory/distiller.rs:1699-1718`**

```text
.remember_authored(&scope, record, author.clone())
```

The Remember branch calls this directly; Update calls supersede_authored, and receipts count quarantine. Neither branch enqueues a proposal. The same direct-write design existed at caf18995.

**`meerkat-mobkit/src/http_console.rs:2908-2922`**

```text
match store.pending_proposals(realm, limit).await {
```

The Pipeline proposal data source is a separate queue, confirming the corrected distinction.

## Independent scope checks

> [
>   "Independently reread every G-001 through G-017 documentary claim and chased its actual implementation; baseline HEAD remained af82b6b3ab34faed9bf3e962d148d55f10dcd1dc.",
>   "Checked the memory-console-ui-proposal's named caf18995 source snapshot for the disputed budget, dedup reset, trust-retier/release, timeline-query and direct-Distiller-write premises. These counterexamples existed at that snapshot; findings are not requests to modernize historical feature inventories.",
>   "scripts/memory-evals --check: passed (4 profiles; 10 Selector, 5 Distiller, 5 Steward, 4 Hygienist, 5 invariant fixtures).",
>   "scripts/memory-evals --help: runnable stage choices are exactly distiller, steward, hygienist.",
>   "scripts/check-memory-bright-line: passed (dependencies and 18 memory-module source files clean).",
>   "In-memory runpy orchestration probe for all three runnable stages: child auth exit 3 was consumed, subsequent invocations selected mock=True, successful score checks returned normally, and controlled deterministic mismatches produced exit 1. Subprocesses/model responses and score results were stubbed; this was not live-model or production-validator execution.",
>   "git ls-files meerkat-mobkit/tests/*eval_harness.rs: exactly the Distiller, Hygienist and Steward harness files.",
>   "Repository status was clean before adjudication and after all source/command checks. Only this session artifact is written."
> ]

## Final scope checks

> [
>   "Read audit-brief.md, the G manifest in audit-scopes.json, full audit-G/adjudication-G/fixes-G, and audit-K/adjudication-K. There is no fixes-K.json artifact: K-002/K-003/K-004 are integrated into fixes-G.json under the brief's ownership assignments.",
>   "Read all 13 G-owned document/comment/prompt paths. Read the three embedded prompts and historical Selector prompt; byte equality establishes identical content in the three corresponding calibration copies. All 13 paths are regular files, not symlinks.",
>   "Read the complete baseline-to-final diffs of all five changed G paths, including segmented reads of the long proposal diff. Independently traced all 20 final documentary corrections to current source and checked their adjacent historical, proposal, and composition caveats.",
>   "git diff --check af82b6b3ab34faed9bf3e962d148d55f10dcd1dc restricted to G paths: passed.",
>   "python3 -B scripts/memory-evals --check: passed (4 profiles; 10 Selector, 5 Distiller, 5 Steward, 4 Hygienist, 5 invariant fixtures).",
>   "scripts/check-memory-bright-line: passed (Cargo dependencies and 18 memory-module source files clean).",
>   "python3 -B scripts/memory-evals --help: supported stage choices are exactly distiller, steward, hygienist. git ls-files found exactly their three eval-harness integration tests.",
>   "Independent in-memory orchestration probes for all three runnable stages: initial child auth exit 3 was consumed; subsequent calls selected mock=True; mock judgment misses returned wrapper exit 0; deterministic mismatches exited 1; auth disappearing after a successful probe exited 2. No child process/model was invoked; score results were controlled stubs, not execution of Rust validators.",
>   "Parsed allowlist entries at the actual '#' comment boundary and compared against baseline: all five non-comment entries unchanged, with no added file/pattern allowance.",
>   "Byte-compared all seven runtime/calibration prompt files against baseline: unchanged. Distiller, Steward, and Hygienist embedded/calibration pairs remain byte-identical.",
>   "Compared all current source panel-method string names to the guide table: exact ten-method equality. Read the six added handlers to verify result fields and realm/limit parsing independently.",
>   "Checked caf18995 directly: it already contains 20/60 KiB budgets, compaction/map resets, steward retier exceptions, quarantine-derived release, direct remember/supersede extraction, and a timeline query without revision/generation/range selectors. Proposal corrections therefore repair false premises rather than rewriting historical feature inventories.",
>   "Manifest-driven Markdown check: balanced fenced blocks for all 13 paths; all 13 local/navigation link occurrences and referenced anchors resolve, including the new current-panel link.",
>   "An initially overbroad repository-code-unchanged assertion encountered unrelated concurrent SDK edits. The correct scoped follow-up passed: meerkat-mobkit/src, console/src, scripts/memory-evals, and scripts/check-memory-bright-line are unchanged from baseline; MemoryPanel.tsx is byte-identical. No finding is attributed to another scope's concurrent edits.",
>   "No rejected adjudications exist among G-001 through G-017 or K-002 through K-004. rejected_items_untouched is therefore true without requiring a rejected-fix exclusion.",
>   "No repository edits, git mutations, commits, dependency installation, delegation, Rust build/test execution, or live-provider calls were performed. Only review-G.json was written as a session artifact."
> ]
