Redaction note (0.9): `Hub` is a pseudonymous, redacted name for a private external memory system, and `<local checkout>/` replaces local checkout paths; see [redaction note](README.md).


================ GAP: Do injected memories re-enter the write paths (echo loops)? In MobKit+meerkat: inject_for_turn prepends the memory block to the actual delivered user message
(identity_first/runtime.rs:2425-2448) and meerkat indexes Message::User text at compaction (types.rs:1201-1203) — so does injected agent-memory text get re-indexed into meerkat
semantic memory and later re-recalled via memory_search, compounding each turn? In Claude Code: does the turn-end extraction fork see relevant_memories system-reminder attachments
and the post-compact summary message as extraction input, or are isMeta/synthetic messages filtered before the fork?

ANSWER:
All evidence gathered. Final report follows.

---

ECHO-LOOP ANALYSIS: DO INJECTED MEMORIES RE-ENTER WRITE PATHS?

=== PART 1: MobKit + meerkat ===

VERDICT: Yes — injected agent-memory text mechanically re-enters meerkat's semantic memory store at compaction (one full copy per discarded injected turn). But the compounding is
LINEAR (store duplication + recall-salience pollution), not geometric, because the two feedback edges that would close a growth loop are both broken: meerkat memory_search results
are ToolResults (excluded from indexing), and MobKit's own agent-memory store has no automatic write-back. One secondary indexable edge exists: compaction summaries.

Write-path trace (all confirmed implemented, not stubbed):

1. Injection is inline in the delivered user content. `IdentityRuntime::send` builds `content_to_deliver` by calling `injector.inject_for_turn(identity, content)` for every
non-Steer send (meerkat-mobkit/src/identity_first/runtime.rs:2425-2439) and delivers it via `bridge.deliver_with_mode(rid, &content_to_deliver, handling_mode)`
(runtime.rs:2442-2446). `inject_for_turn` (agent_memory.rs:389-421) calls `prepend_memory_injection` (agent_memory.rs:854-871), which physically concatenates the formatted memory
block into the user text: `format!("{injection}\n\nCurrent user message:\n{text}")` for Text, or a prepended `ContentBlock::Text` for Blocks. There is no
side-channel/system-prompt variant on the per-turn path — the memory block IS user-message content. Size: up to 8 records default / 64 max (agent_memory.rs:25,27), each ≤160B
title + ≤2048B body (agent_memory.rs:37-38), so ~18KB/turn at defaults.

2. The injected ContentInput is exactly what persists in the meerkat transcript. Chain: `deliver_with_mode` → `submit_internal_bridge_work` → `handle.submit_work_with_mode(...,
WorkSpec::new(content.clone(), WorkOrigin::Internal), ...)` (meerkat-mobkit/src/identity_first/bridge.rs:971-1034, 136-158) → mob shell forwards `spec.content` verbatim into
`SubmitWorkPayload` (meerkat/meerkat-mob/src/runtime/handle.rs:2440-2466, `content: spec.content` at :2459) → actor dispatch builds `StartTurnRequest { prompt: content, ... }`
(meerkat/meerkat-mob/src/runtime/actor.rs:18400-18415 autonomous, 18466-18481 turn-driven) → meerkat runner commits it as a user message:
`self.session.push(Message::User(user_message))` built from the full `user_input` (meerkat/meerkat-core/src/agent/runner.rs:1232-1239). No stripping of the injected prefix
anywhere on this path.

3. Compaction re-indexes it. `index_compaction_discards` is called on every successful compaction (meerkat/meerkat-core/src/agent/state.rs:1265) and indexes every discarded
message via `message.indexable_content()` into a session-scoped memory store (`MemoryIndexScope::for_session`, state.rs:1362-1431, scope at :1367). `indexable_content()`
classifies `Message::User(u)` as `Indexable(u.text_content())` unconditionally (meerkat/meerkat-core/src/types.rs:1201-1203) — it does not inspect content or `transcript_role`.
There is NO filter for the injection marker: `grep mobkit_memory_observation` over meerkat-core and meerkat-memory returns zero hits. So each discarded injected turn deposits a
full copy of the memory block (header + `<mobkit_memory_observation>` items, agent_memory.rs:784-811) into meerkat semantic memory as a `MemorySource::Compaction` entry.

4. Re-recall and (non-)compounding:
- meerkat `memory_search` (meerkat/meerkat-memory/src/tool.rs, TOOL_NAME at :20, session-scoped at :94-99) will hit those copies — recall-quality pollution is real, and grows by
one copy per compacted injected turn. But results return as tool results; `Message::ToolResults` is `Excluded(MemoryIndexExclusion::ToolResults)` (types.rs:1220-1222), so recalled
text is never itself re-indexed. No mechanical index→recall→index loop.
- MobKit's `inject_for_turn` recall queries the MobKit `AgentMemoryProvider` (markdown file store, agent_memory.rs:198-318; only other production impl is caller-provided), NOT
meerkat's memory store — different store, so meerkat-indexed copies never feed back into the injection selector. The recall query is derived from the raw incoming `content`
pre-injection (runtime.rs:2431 passes `content`, and agent_memory.rs:394 uses `content.text_content()`), so prior injections don't skew the query either.
- MobKit store writes are explicit-only: `remember_agent_memory` via RPC `mobkit/agent_memory/remember` (rpc.rs:1966-2010) and console (http_console.rs:4186), both operator
surfaces — there is no transcript-extraction write-back and no agent-facing remember tool. So injected text cannot mechanically re-enter the MobKit store.

5. Secondary indexable edge: compaction summaries. `DefaultCompactor::rebuild_history` injects the summary as `Message::User(UserMessage::compaction_summary(...))`
(meerkat/meerkat-session/src/compactor.rs:194-197) carrying `TranscriptUserRole::CompactionSummary` (types.rs:2171-2178), and discards everything before the retained-turn window
including prior summaries (compactor.rs:226-229). Since `indexable_content()` ignores `transcript_role` (types.rs:1201-1203; the only consumer of `is_compaction_summary` is
session_store.rs:701), a discarded summary — which may contain LLM-distilled injected-memory content — is itself indexed at the next compaction. Model-mediated content, mechanical
indexing.

6. Exempt paths: Steer bypasses injection (runtime.rs:2425-2427); `dispatch()` delivers `input.content` raw with no injection (runtime.rs:~2524 `bridge.deliver(rid,
&input.content)`); build-time injection goes to `draft.additional_instructions` (agent_memory.rs:473-479) → system prompt → `Message::System` excluded from indexing
(types.rs:1214-1216).

Net MobKit failure shape: per compaction cycle, N discarded turns × ~18KB injected block = duplicate near-identical entries in the session-scoped meerkat store, progressively
dominating memory_search results for any query overlapping the injected records' vocabulary. Not exponential, but monotonic within a session lifetime, and unbounded for a
long-lived identity-first agent whose session compacts many times.

=== PART 2: Claude Code ===

VERDICT: No filtering — the extraction fork sees BOTH relevant_memories system-reminder attachments AND the post-compact summary as extraction input. Neither isMeta nor
isCompactSummary is filtered before or inside the fork; this is by design (byte-identical prefix for prompt-cache sharing). Loop damping is entirely model-mediated (prompt
instructions + existing-memory manifest), plus write-side idempotence of the file store.

Evidence:

1. Fork input is the parent transcript verbatim. `runExtraction` builds `cacheSafeParams = createCacheSafeParams(context)` (src/services/extractMemories/extractMemories.ts:372),
which sets `forkContextMessages: context.messages` with no filtering (src/utils/forkedAgent.ts:131-141). `runForkedAgent` then does `initialMessages = [...forkContextMessages,
...promptMessages]` (forkedAgent.ts:524) — the comment at :520-523 explicitly says not to filter, to keep the cache prefix identical. `grep isMeta forkedAgent.ts` → zero hits. The
stated design: "a perfect fork of the main conversation that shares the parent's prompt cache" (extractMemories.ts:8-9).

2. relevant_memories attachments are model-visible in that list. They live in `context.messages` as `type: 'attachment'` messages (src/utils/attachments.ts:2251-2266 scans exactly
that shape). At API-payload time, `normalizeMessagesForAPI` (src/utils/messages.ts:1989) renders them: the pre-switch filter at :2066-2074 drops only progress / non-local-command
system / synthetic-API-error messages; the `case 'attachment'` at :2269-2290 calls `normalizeAttachmentForAPI`, whose `case 'relevant_memories'` (messages.ts:3708-3722) produces
system-reminder-wrapped `createUserMessage({..., isMeta: true})` per memory file. isMeta user messages pass through normalization to the API (e.g. :1620-1623 merely skips
ID-tagging them). The fork runs the same `query()` pipeline (forkedAgent.ts:545-556), so the extraction agent's context contains the full injected memory contents.

3. Post-compact summary is also in the fork input. The summary is `createUserMessage({ content: ..., isCompactSummary: true, isVisibleInTranscriptOnly: true })`
(src/services/compact/compact.ts:614-624; also :1034-1043 and sessionMemoryCompact.ts:479-480). `isVisibleInTranscriptOnly` is a UI-rendering flag only (messages.ts:4675); nothing
in normalizeMessagesForAPI or forkedAgent filters `isCompactSummary`. It is a `type: 'user'` message, so it also counts in `countModelVisibleMessagesSince`
(`isModelVisibleMessage` = type user||assistant, extractMemories.ts:78-80). Worse, compaction typically removes the cursor message, triggering the explicit fallback "if sinceUuid
was not found (e.g., removed by context compaction), fall back to counting all model-visible messages" (extractMemories.ts:103-108) — so post-compaction, the summary is squarely
inside the "most recent ~N messages" window the prompt directs extraction at (prompts.ts:35, 41).

4. What actually dampens the loop (all model-mediated, none mechanical):
- The extraction prompt pre-injects an existing-memory manifest ("Check this list before writing — update an existing file rather than creating a duplicate", prompts.ts:30-33) and
repeats "Do not write duplicate memories" (prompts.ts:65, 81, 124, 140). Re-extracting an injected memory is steered into an update of the same topic file — idempotent-ish because
the store is path-keyed files, not append-only entries.
- `WHAT_NOT_TO_SAVE_SECTION` (src/memdir/memoryTypes.ts:183-195) excludes "current conversation context" but has no clause about system-reminder or already-injected memory content.
- `hasMemoryWritesSince` mutual exclusion (extractMemories.ts:121-148, 348-360) only guards main-agent direct writes; irrelevant to injection echo.
- Attachment-typed messages don't increment `newMessageCount` (type 'attachment' fails the user/assistant check) — the count ignores them but the content is still fully visible to
the fork.
- Surfacing side: `collectSurfacedMemories` de-dups paths and byte-throttles re-surfacing within a session (attachments.ts:2244-2266), but its own comment notes compaction resets
this "so re-surfacing is valid again" — i.e., post-compact, a memory can be re-injected AND the fork can re-see it.

Net Claude Code failure shape: injected memory content and compact summaries are legitimate mechanical extraction input every turn; the system depends on (a) the
manifest+no-duplicates prompt making re-saves converge to updates of the same file, and (b) the file store being keyed by topic path, to keep the loop a fixed point instead of
growth. There is no `isMeta`/synthetic pre-fork filter — asserting one exists would be wrong.

=== DESIGN TAKEAWAYS FOR MOBKIT MEMORY SYSTEM ===

1. meerkat's typed exclusion enum (`MemoryIndexExclusion`, types.rs:1237-1247) is the right shape but has no variant for "injected/synthetic user content" — MobKit's inline
user-text injection defeats it. Options: deliver injections as `Message::SystemNotice` / a typed transcript role that `indexable_content()` excludes, or strip a typed marker at
`index_compaction_discards`. Today `TranscriptUserRole` exists but indexing ignores it — that's the one-line seam (types.rs:1201-1203).
2. Claude Code's echo-tolerance comes from a path-keyed, update-in-place store + manifest-in-prompt; an append-only or entry-keyed store (like meerkat's compaction index) gets
linear duplication instead.
3. Both systems break the geometric loop at the same place: recall output lands in an indexing-excluded message class (ToolResults / prompt-only manifest). Any MobKit design that
injects recall as plain user text into a transcript that is later auto-indexed re-creates the MobKit-side duplication bug.


================ GAP: What actually happens to meerkat session semantic memory across MobKit identity lifecycle operations (respawn, reset, live-respawn rebind, resume)? Meerkat
memory is scoped to exactly one SessionId with no delete API and monotonic growth; MobKit respawn/reset create or rebind sessions while the identity persists. Does a respawned
identity get a new session id (orphaning all previously indexed compaction discards forever in the shared realm memory.sqlite3), does resume reuse the old session id (memory
survives), and does anything ever clean up orphaned rows?

ANSWER:
All evidence gathered. Final report follows.

---

## Meerkat session semantic memory across MobKit identity lifecycle operations

### Baseline meerkat facts (confirmed in code)

**Scope is exactly one SessionId, no delete API, monotonic growth — all confirmed.**

- `MemoryOwner` wraps a single `session_id`; `includes()` is strict equality on it — `(operator-retained path)/meerkat/meerkat-core/src/memory.rs:10-27`. Both `MemorySearchScope` and
`MemoryIndexScope` are just `MemoryOwner` wrappers (memory.rs:128-151, 155-178).
- The `MemoryStore` trait has exactly three methods: `index_scoped`, `index_scoped_batch`, `search` — **no delete/prune/expire API exists** (memory.rs:404-434).
- `HnswMemoryStore` persists into one shared SQLite file `<dir>/memory.sqlite3` (`(operator-retained path)/meerkat/meerkat-memory/src/hnsw.rs:281`) with in-memory per-session HNSW graphs
(`indices: HashMap<SessionId, ScopedIndexState>`, hnsw.rs:239). Inserts are append-only (hnsw.rs:419-608). The **only** `DELETE FROM memory_*` statements in the crate are the
rollback-repair of a partially failed batch (hnsw.rs:548-559) — not a cleanup facility.
- **Store-path wiring** (`(operator-retained path)/meerkat/meerkat/src/factory.rs:5411-5472`): inside `build_agent`, when memory is effective for the realm (`effective_memory_for_realm`,
factory.rs:891-897; disabled only for in-memory recovery backends, factory.rs:885-889), the factory calls `HnswMemoryStore::open(self.store_path.join("memory"))`
(factory.rs:5419-5420) and pins the `memory_search` tool to `MemorySearchScope::for_session(session_id.clone())` (factory.rs:5427), where `session_id = session.id().clone()` of
the session being built/resumed (factory.rs:~5386). Note `open()` runs **per agent build**, not once per process: every build re-scans *every* row in the shared DB (unfiltered
`SELECT`, hnsw.rs:319-335), re-embeds every text (hnsw.rs:344), and constructs an in-memory HNSW graph per session id found — including orphaned sessions nothing will ever search
(hnsw.rs:348-354). Fail-closed if the store can't open (factory.rs:5451-5457).

### MobKit capability pass-through

- `<local checkout>/meerkat-mobkit/.claude/worktrees/memory-system/meerkat-mobkit/src/mob_handle_runtime.rs:2412-2417`:
`AgentFactory::new(&store_path)...comms(caps.comms).memory(caps.memory)`. `CapabilityFlags::default()` has `memory: true` (mob_handle_runtime.rs:2840, 2851). The persistent
runtime variants do the same pass-through (lines 2600, 2700). So every MobKit-built member gets session-scoped semantic memory against `<store_path>/memory/memory.sqlite3` — one
shared realm DB for all identities/generations under that runtime's store path.

### respawn — new session id, old memory orphaned

Two layers interact:

1. `IdentityRuntime::respawn` (`<local checkout>/meerkat-mobkit/.claude/worktrees/memory-system/meerkat-mobkit/src/identity_first/runtime.rs:2708-2820`) is itself non-destructive:
it re-fences the lease and reuses the **same** `ContinuityRecord` — same `session_id`, generation "does NOT advance" (comment at 2707); it only re-registers runtime state for the
existing session id (`refresh_existing_session_runtime_state`, runtime.rs:2313-2336).
2. But the shipped `mobkit/respawn` RPC then always performs a live member refresh: `rpc.rs:2937` `identity_rt.respawn` → `rpc.rs:2939` `respawn_rpc_runtime_member_id` →
`rpc.rs:4376` `handle.respawn(member_id, None)`. Meerkat-mob's `handle_respawn` (`<local checkout>/meerkat/meerkat-mob/src/runtime/actor.rs:14189+`) is a retire-plus-replacement:
it retires the old member ("archives the session", actor.rs:14388-14393), discards the spec's `launch_mode` (actor.rs:14327), builds a fresh `CreateSessionRequest` with no
`resume_session` (actor.rs:14553), and `admit_bridge_session_for_spawn` therefore mints `SessionId::new()` (actor.rs:801-817, admitted at 14570-14574). The RPC reads the new live
session id and rebinds continuity to it via `rebind_session_after_live_respawn` (rpc.rs:2950-2972). The console does the identical dance (http_console.rs:5233;
console_aggregator/mod.rs:2148, 2200 — the "self-heal stale durable alias" paths).

**Result: a respawned identity's `memory_search` scope becomes the new session id. Every compaction discard indexed under the old session id in the shared memory.sqlite3 is
permanently unreachable.** `rebind_session_after_live_respawn_locked` (runtime.rs:2875-3033) only rewrites `record.session_id` (2933-2934) and `unregister_session_runtime_state`
for the previous session (2989-3000) — which merely drops the continuity-store session-projection mapping (bridge.rs:1246-1257); it never touches the memory store.

Same rotation happens implicitly on **delivery repair**: `repair_member_for_delivery` calls `handle.respawn` (bridge.rs:507), and a rotated delivered session id triggers
`reconcile_delivered_session_locked` → rebind (runtime.rs:2836-2873, warn "identity bridge delivery returned a rotated session; rebinding continuity").

### reset — new session id by design, old memory orphaned

`IdentityRuntime::reset` (runtime.rs:3045+) mints `SessionId::new()` at runtime.rs:3113, advances the generation (3112), `bridge.create_session` spawns a fresh member
(bridge.rs:782-806) whose actual bridge session id is resolved from the mob (bridge.rs:804) and written back (runtime.rs:3225-3226). Old-session cleanup is a fire-and-forget task
`spawn_old_bridge_cleanup_after_reset` (runtime.rs:3838-3885) that explicitly "only clears stale session projection" (3854) — i.e., unregisters the old session from the continuity
adapter. No memory rows are removed.

### resume — same session id, memory survives (except fallback)

Lazy materialization (runtime.rs:1181-1292) calls `bridge.resume_session` with the continuity record's session id; the bridge sets `MemberLaunchMode::Resume { bridge_session_id:
session_id }` (bridge.rs:824-826 external, 846-848 session-backed). Meerkat-mob's resume fast-path either adopts a still-active bridge session with the same id
(actor.rs:8955-9017) or loads the persisted session and sets `config.resume_session = Some(resumed_session)` (`(operator-retained path)/meerkat/meerkat-mob/src/build.rs:355`), so
`admit_bridge_session_for_spawn` returns the **existing** id (actor.rs:811-812). The factory then re-opens the shared store and scopes `memory_search` to that same session id
(factory.rs:5427). **True resume preserves semantic memory across process restarts.**

Caveat: resume has a fresh-spawn fallback (`ResumeSessionOutcome::FreshSpawned`, bridge.rs:860-896, e.g. when the old session's comms identity is still claimed after an in-process
restart). The runtime then rebinds `record.session_id` to the fresh id (runtime.rs:1286-1290, warn at 1271-1276 "lazy identity materialization fresh-spawned after typed resume
fallback") — orphaning the old session's memory exactly like respawn.

### retire / delete_identity

`retire_member` (bridge.rs:1063-1085) and `delete_identity` (runtime.rs:3470-3563) retire the mob member, unregister session runtime state, and delete the continuity record.
Neither path references the memory store. A fully deleted identity leaves all its memory rows in memory.sqlite3 forever.

### Does anything ever clean up orphaned rows? No.

- No delete API on the `MemoryStore` trait (memory.rs:404-434); the only implementations are `HnswMemoryStore` and test-oriented `SimpleMemoryStore` (meerkat-memory/src/lib.rs:14).
- The only DELETEs in `hnsw.rs` are same-batch rollback repair (548-559). No TTL, GC, vacuum, or per-session drop exists in meerkat or MobKit.
- Cost of orphans is not just disk: because `HnswMemoryStore::open` runs inside every `build_agent` (factory.rs:5420) and rebuilds *all* per-session graphs by re-embedding *every*
row (hnsw.rs:300-355), orphaned rows tax **every subsequent agent build/resume in the realm** with CPU (embedding), RAM (dead HNSW graphs), and startup latency — monotonically
growing with each respawn/reset generation.

### Side note

MobKit already ships a separate identity-keyed memory layer (`identity_first/agent_memory.rs`, commit b7a8d228): markdown records keyed by `AgentIdentity`, injected into build
drafts/turns at materialization (agent_memory.rs:1-5, 389, 458-480). It is entirely disjoint from meerkat's session-scoped semantic memory — it survives lifecycle ops precisely
because it never touches SessionId — confirming there is currently **no** bridging of meerkat compaction memory across identity generations.

### Survival matrix (summary)

| Operation | Meerkat session id | Semantic memory |
|---|---|---|
| Materialize/resume (true resume) | reused | survives |
| Resume fresh-spawn fallback | rotated | orphaned |
| `mobkit/respawn` RPC / console respawn | rotated (always, via mob-level retire+spawn) | orphaned |
| Delivery-repair respawn | rotated | orphaned |
| `reset` | rotated by design (new generation) | orphaned |
| `retire` / `delete_identity` | session gone | rows persist forever, unreachable |
| Orphan cleanup | — | none exists; orphans also slow every future agent build via full-DB re-embed at `HnswMemoryStore::open` |


================ GAP: How much duplicate memory text accumulates in a MobKit agent's context over a session? The build-time customizer injects a recall block into the system
prompt AND the per-turn injector prepends a recall block to every non-Steer user message using the same provider/config — do the same top-scoring records appear in both, and is
there ANY cross-turn suppression (already-injected dedup, cumulative byte budget, or turn throttle) analogous to Claude Code's 20KB/turn–60KB/session ladder, or does a stable
top-8 get re-injected verbatim (up to ~16KiB) into every persisted user message?

ANSWER:
All evidence gathered. Here is the report.

---

# MobKit agent-memory duplication analysis (worktree: memory-system)

## 1. The two injection paths share provider AND config — confirmed at three wiring sites

**Build-time (system prompt):** `AgentMemoryCustomizer::customize_build` — `meerkat-mobkit/src/identity_first/agent_memory.rs:448-481`. Calls `recall_for_injection(&self.provider,
&self.config, AgentMemoryRecallRequest{ ..., max_entries: self.config.max_entries })` (lines 458-468) and pushes `format_memory_injection(...)` into
`draft.additional_instructions` (474-478), i.e. into the durable system-prompt side of the build.

**Per-turn (user message):** `AgentMemoryRuntimeInjector::inject_for_turn` — `agent_memory.rs:389-421`. Same `recall_for_injection` with the same `self.config.max_entries`
(401-412), same `format_memory_injection` (417-420), result prepended to the outgoing `ContentInput` via `prepend_memory_injection` (854-871): `"{injection}\n\nCurrent user
message:\n{text}"` for Text, or an extra leading Text block for Blocks.

**Wiring — same provider object, same config value, in every production path:**
- `unified_runtime/builder.rs:588-598`: `agent_memory_injector` built from `provider.clone()` + `self.agent_memory_config.clone().unwrap_or_default()`; `composed_agent_customizer`
(builder.rs:241-253) wraps the *same* provider with the *same* `self.agent_memory_config.clone().unwrap_or_default()`. Injector installed at builder.rs:759 (`set_agent_memory`).
- `src/bin/rpc_gateway.rs:2634-2657`: customizer (`AgentMemoryCustomizer::wrap(..., agent_memory.config.clone())`) and injector (`AgentMemoryRuntimeInjector::new(provider,
agent_memory.config.clone())`) built back-to-back from identical inputs.
- `src/rpc.rs:5271` and `src/http_console.rs:9952,10066` are test wiring, same pattern.

**Per-turn trigger point:** `identity_first/runtime.rs:2425-2439` — inside the send path, `if handling_mode == HandlingMode::Steer` the content passes through untouched; **every
non-Steer send** goes through `inject_for_turn`. The mutated `content_to_deliver` is then handed to `bridge.deliver_with_mode(rid, &content_to_deliver, handling_mode)`
(runtime.rs:2443-2444), i.e. the memory block is baked into the ContentInput *before* delivery, so it becomes part of the user message the meerkat session persists in its
transcript. There is no post-delivery stripping anywhere in mobkit.

## 2. Do the same records appear in both? Depends on selection mode, but yes in the steady state

`MarkdownAgentMemoryStore::recall_blocking` (`agent_memory.rs:270-303`) is fully deterministic:
- **`Always` mode:** sort by `updated_at_ms` desc, tiebreak `created_at_ms` desc, `truncate(max_entries)` (295-301). The build-time query and every per-turn query return the
**identical top-N** (query text is ignored in Always mode). Guaranteed verbatim duplication: once in the system prompt + once in every non-Steer user message.
- **`Contextual` mode (default, agent_memory.rs:47-48):** the *queries* differ — build-time uses identity/profile/peers/edges/labels terms (`build_query_terms` 813-830,
`build_query_text` 832-852); per-turn uses the compacted user-message text (389-397). So build vs turn sets can diverge. But scoring (`record_relevance_score` 972-999, threshold
`MIN_CONTEXTUAL_RELEVANCE_SCORE = 2` at line 29, sort at 287-292) is deterministic over a store that only changes on writes: **if the user stays on one topic, every turn
re-selects and re-injects the exact same top-8 verbatim**, and any record matching both the identity/label vocabulary and the conversation topic appears in the system prompt too.

## 3. Cross-turn suppression: NONE — confirmed by structure, not just grep

- `AgentMemoryRuntimeInjector` has exactly two fields: `provider` and `config` (`agent_memory.rs:367-371`). No session map, no turn counter, no injected-ids set, no interior
mutability.
- `inject_for_turn` takes only `(identity, content)` (389-393) — it doesn't even receive a session id or turn number, so it *cannot* dedup across turns without a redesign of its
signature.
- `recall_for_injection` (523-550) is a pure timeout/failure-policy wrapper; no state.
- Grep across `agent_memory.rs` + `runtime.rs` for `dedup|already_injected|injected_ids|seen|suppress|throttle|budget|cumulative`: the only hits are unrelated (`suppress_until` at
runtime.rs:382/944/966 is materialization-failure backoff; `seen` at runtime.rs:2285/3701 is session-id and roster-spec dedup). Nothing tracks memory injection.
- There is also no total-size cap on the formatted injection block. The only caps in `format_memory_injection` (784-811) are **per-record**: title truncated to
`MAX_INJECTED_TITLE_BYTES = 160` (line 37), body to `MAX_INJECTED_BODY_BYTES = 2_048` (line 38), via `truncate_utf8_boundary` (1011-1020). Note the truncation happens **before**
XML escaping (799-807), so a body full of `<`/`&` can expand up to ~4-5x past 2048B in the rendered output (`escape_xml_text` 1052-1057). The count cap is `config.max_entries`,
default 8 (`DEFAULT_MAX_ENTRIES`, line 25), clamped to at most `MAX_MEMORY_ENTRIES = 64` by `normalize_config` (505-521). The test at agent_memory.rs:2089-2106 asserts only
per-record truncation of a single record — no aggregate-size test exists.

## 4. Quantified accumulation

Per-record injected max ≈ 160B title + 2,048B body + ~75B XML wrapper ≈ 2.3KB (pre-escaping). Header preamble ≈ 230B.

- **Default config (max_entries=8):** up to ~18.5KB per non-Steer turn (the "~16KiB" in the question is the 8×2048 body budget alone), re-injected **every turn**, plus one copy in
the system prompt. After T turns of a stable-topic session: `~18.5KB × (T+1)` of largely identical text live in context and persisted in the transcript. 20 turns ≈ ~390KB (~100K
tokens) of duplicated memory text.
- **Max config (max_entries=64):** up to ~147KB per turn.
- The persisted transcript compounds this: because injection mutates the ContentInput before `deliver_with_mode`, every historical user message in the session permanently carries
its own copy of the recall block; nothing rewrites or collapses them.

## 5. Contrast: Claude Code's budget ladder (`(operator-retained path)/claude-code/src/utils/attachments.ts:269-289`)

Claude Code enforces a three-level ladder for its analogous memory-surfacing feature:
1. **Per-file cap:** `MAX_MEMORY_BYTES = 4096` (plus `MAX_MEMORY_LINES = 200`) — comment explicitly notes line caps alone don't bound size.
2. **Per-turn cap:** 5 files × 4KB = **20KB/turn** (comment at ~271-275).
3. **Cumulative session cap:** `RELEVANT_MEMORIES_CONFIG.MAX_SESSION_BYTES = 60 * 1024` — "once hit, stop prefetching entirely"; budget is "~3 full injections"; the counter is
computed by **scanning messages** rather than side-state, so compaction naturally resets it (comment at ~279-289). It also throttles other recurring attachments
(`TURNS_BETWEEN_ATTACHMENTS: 5`, `TURNS_BETWEEN_REMINDERS: 10` at ~254-267).

MobKit has an analog of level 1 only (per-record 160B/2048B truncation). It has **no level 2 aggregate-per-turn byte cap** (only the count cap, worst case ~18.5KB default / ~147KB
at max_entries=64), and **no level 3 at all** — no cumulative session budget, no already-injected dedup, no turn throttle, and no message-scanning reset mechanism. Nothing in the
codebase is stubbed or planned for this either (no TODO/comment referencing dedup or session budgets in `agent_memory.rs` or `runtime.rs`).

## 6. Bottom line

Implemented behavior: with the default `Contextual` selection and a topically stable session, a stable top-8 (≤ ~18.5KB rendered) is re-selected deterministically and re-injected
verbatim into **every persisted non-Steer user message**, on top of a build-time copy in the system prompt, with zero cross-turn suppression of any kind. With `Always` selection
the build-time and per-turn sets are provably identical. The only session-lifetime bound is the store-side retention (512 records / 8MB file, `agent_memory.rs:35-36, 649-668`),
which bounds what *can* be recalled, not what accumulates in context. Duplication grows linearly with turn count, unbounded per session.

Key files:
- `<local checkout>/meerkat-mobkit/.claude/worktrees/memory-system/meerkat-mobkit/src/identity_first/agent_memory.rs` (customizer 448-481; injector 389-421; format 784-811; caps
25-38; normalize 505-521; recall 270-303)
- `<local checkout>/meerkat-mobkit/.claude/worktrees/memory-system/meerkat-mobkit/src/identity_first/runtime.rs` (per-turn hook 2425-2444)
- `<local checkout>/meerkat-mobkit/.claude/worktrees/memory-system/meerkat-mobkit/src/unified_runtime/builder.rs` (dual wiring 241-253, 588-598, 759)
- `<local checkout>/meerkat-mobkit/.claude/worktrees/memory-system/meerkat-mobkit/src/bin/rpc_gateway.rs` (dual wiring 2634-2657)
- `<local checkout>/cc/claude-code/src/utils/attachments.ts` (contrast ladder 254-289)


================ GAP: How does Hub truth maintenance actually DETECT a conflict — what exactly goes into the deterministic slot_key (subject + predicate + security_hash?
object normalization? temporal window?), what condition places two assertions into the same conflict_group (any differing object in one slot, or value-type-specific comparison?),
and how does supersede_by_recency pick the winner and close the loser's validity window? The reports name the ConflictPolicy variants but never the detection/resolution mechanics,
which is the piece MobKit's signal-only conflict ledger lacks.

ANSWER:
HUB TRUTH MAINTENANCE — CONFLICT DETECTION AND RESOLUTION MECHANICS
(all paths under `<local checkout>/hub`; line numbers from current working tree)

=====================================================================
1. THE DETERMINISTIC slot_key — EXACT COMPOSITION
=====================================================================

Function: `compute_slot_key` — crates/pipeline/src/truth_maintenance/mod.rs:689-716. It is a plain "::"-joined string, NOT a hash (parseable back via `parse_slot_key`,
mod.rs:735-778):

- Attribute slots: `subject_entity_id :: predicate_id :: [context] :: security_hash` (context optional; 3 or 4 parts)
- Rel slots: `subject_entity_id :: object_entity_id :: predicate_id :: security_hash` (always 4 parts)

security_hash = hex SHA-256 over the record's Security envelope: policy_id bytes + level (big-endian bytes) + sorted labels + sorted handling (`compute_security_hash`,
mod.rs:801-820). Detection filters candidate claims to those with an EQUAL security hash (mod.rs:1428, 1543), so the same fact under a different ABAC envelope is a different slot,
never a conflict.

Critical caveats found in code, not in the reports:
- The `context` component is DEFINED but NEVER POPULATED: both detection call sites pass `None` (mod.rs:1410-1416 for rels, 1530-1536 for attributes). "context_bucket" exists only
in the spec comment (mod.rs:687) and the parser's tolerance for 4-part attribute keys. So in practice attribute slot_key = subject::predicate::security_hash.
- There is NO object/value component and NO object normalization in the rel/attr slot_key, and NO temporal window. Predicate strings are used verbatim (`rel_type` / `attr_type` as
promoted, i.e. raw predicate_text from extraction — inline_promotion.rs:341, 374); alias canonicalization is handled by separate staleness/versioning machinery
(semantics_version/alias_version on TruthSlot, mod.rs:3073-3084), not by the key.

EVENT slots are a separate, hashed key with real normalization (`compute_event_slot_key`, mod.rs:1206-1216): hex SHA-256 of `participants_key | day_bucket | title_key` where:
- participants_key = sorted+deduped subject entity ids joined "|"; fallback for participant-less events = sha256 of sorted evidence refs, prefixed "evidence:" (mod.rs:1150-1191)
- day_bucket = `time_start` formatted YYYY-MM-DD, "unknown" if absent (mod.rs:1193-1197) — this is the only temporal window anywhere in slot keying, and it exists only for events
- title_key = title lowercased, split on non-alphanumerics, stopwords removed (26-word list, mod.rs:1125-1128), tokens sorted, first 6 joined "|" (mod.rs:1130-1148)

=====================================================================
2. WHAT PUTS TWO ASSERTIONS IN ONE conflict_group
=====================================================================

Detection is `execute_tm_detect_conflicts_with_storage` (mod.rs:1395-1657). The condition is purely CLAIM-ROW COUNT PER SLOT — there is no value comparison of any kind, not
value-type-specific, not even equality:

- For an attribute trigger: claims = ALL non-soft-deleted attribute rows with same subject + attr_type (`AttributeRepository::find_for_slot`,
crates/storage/src/attribute_repository.rs:398-439 — the SQL filters only space_id, subject, attr_type, deleted_at; `value` is never inspected), then filtered to equal
security_hash (mod.rs:1543). `claim_count > 1` ⇒ conflict group (mod.rs:1586-1603). Consequence: two attribute rows with IDENTICAL values also form a "conflict", because promotion
always creates a fresh attribute row per new assertion (inline_promotion.rs:328-359; only exact-duplicate assertions are stopped earlier by assertion_key idempotency,
inline_promotion.rs:94-132). Differing values conflict; identical-value duplicates conflict too.
- For a rel trigger: claims = all non-deleted rel rows with the EXACT SAME (subject, object, rel_type) triple (`RelRepository::find_for_slot`,
crates/storage/src/rel_repository.rs:949-993) + equal security hash. Because the object is part of the slot_key, DIFFERING OBJECTS NEVER CONFLICT: "alice works_at Acme" and "alice
works_at Mojang" are two different slots and two truth slots; `Cardinality::One` exists in PredicateSemantics (types/src/records/predicate.rs:453-454) but is never enforced in
detection — it is only interpolated into the LLM prompt (mod.rs:1975-1976). Rel conflict groups can therefore only contain duplicate rows of one triple, which promotion itself
mostly prevents via `merge_or_create_rel` (evidence merged into one row, confidence = max — inline_promotion.rs:414-456). This is a genuine design gap for the "same
subject+predicate, different object" case.
- EventLike predicates skip conflict grouping entirely and go straight to direct commit (mod.rs:1451-1467, 1567-1583; `predicate_is_event_like`, mod.rs:1695-1708).
- Event records: conflicts_detected is counted when >1 Approved/Candidate events share an event slot_key, but NO conflict group record is created — events are always resolved
inline by a deterministic ranking (status weight desc, confidence desc, evidence count desc, updated_at desc, claim_id asc — `resolve_event_group`, mod.rs:1251-1280) and emitted
as direct commits (mod.rs:1330-1386).

Group bookkeeping: the unresolved group's RecordId is deterministic — UUID carved from sha256(space_id::slot_type::slot_key::"unresolved") (mod.rs:1787-1804) — so repeated
detections upsert the SAME group (`upsert_unresolved_conflict_group` → repo upsert by id, conflict_group_repository.rs:239-310), setting claim_ids to the current full claim list.
When claim_count == 1 but an unresolved group already exists, the new claim id is UNION-merged into it (`merge_unresolved_claim_ids`, conflict_group_repository.rs:315-369; returns
None if the group is resolved/missing, in which case the single claim direct-commits). claim_count == 0 with a deleted trigger closes the group ("All claims deleted") and
clears/stales the slot (`handle_deleted_slot`, mod.rs:1659-1693). Assertion/Entity/Predicate/Revision triggers do NOT run detection — they mark truth slots stale for tm_recompute
(mod.rs:954-982, 1010-1071).

ConflictGroup record shape: crates/types/src/records/truth.rs:91-122 (slot_key, slot_type, claim_ids: Vec<String>, resolved, resolution_rationale, detected_at, resolved_at).
TruthSlot: truth.rs:145-199 (current_values: Vec<CurrentValue{claim_id, confidence, status, evidence}>, conflict_group link, stale/stale_reason, semantics_version/alias_version,
security_hash).

=====================================================================
3. HOW supersede_by_recency PICKS THE WINNER
=====================================================================

Policy dispatch: `execute_tm_resolve_slot` (mod.rs:2096-2160) loads PredicateSemantics from the predicate registry; if absent, hardcoded fallback gives "has title"
SupersedeByRecency (mod.rs:780-790, 2134-2136), everything else defaults to AllowParallel (ConflictPolicy Default, truth.rs:16-28), and a `predicate_conflict_no_semantics` system
event is emitted when >1 claims have no semantics (mod.rs:2116-2128).

Winner selection (`resolve_supersede_by_recency`, mod.rs:2514-2530): `claims.iter().max_by_key(|c| c.timestamp)` — newest timestamp wins, confidence is NOT considered, and Rust's
max_by_key tie-break gives the LAST claim in iteration order on equal timestamps (claims arrive in claim_id order from the group / created_at order from storage).

The timestamp is semantics-aware (`claim_timestamp_for_semantics`, mod.rs:3863-3894, via `build_claim_metadata`, mod.rs:3896-3916): take the MAX `time_value` among the claim's
EvidenceRefs whose `time_basis` matches `semantics.recency_basis`, then walk `semantics.fallback_order` (default [SourceEventTime, ObservedAt, AssertedAt], predicate.rs:509-527),
then max of any evidence time, finally the row's created_at. Evidence time_basis is stamped at promotion: SourceEventTime if the extraction carried an event time, else
ObservedAt=now (inline_promotion.rs:311-326).

=====================================================================
4. HOW THE LOSER'S VALIDITY WINDOW IS CLOSED (tm_commit)
=====================================================================

`execute_tm_commit_with_storage` (mod.rs:3272-3813):
1. winner_time = claim_timestamp_for_semantics(winner) — computed only when there is exactly one winner and not needs_review (mod.rs:3625-3636).
2. Status assignment (mod.rs:3603-3617): winner → ClaimStatus::Approved; every other claim id in the commit → ClaimStatus::Superseded; needs_review → ALL claims Disputed and no
windows touched.
3. Window mutation per claim (mod.rs:3651-3657):
   - Loser: `apply_supersession_window` (mod.rs:3234-3252) — window.start defaults to the loser's OWN claim timestamp if it had no window; window.end = winner_time, but only if
the existing end is absent or LATER than winner_time (monotone tightening; never reopens). So the loser's validity interval is [its own recency timestamp, winner's recency
timestamp].
   - Winner: `apply_winner_window` (mod.rs:3254-3262) — start = winner_time only if unset; end left as-is (open-ended = "still current"). ValidityWindow is {start: Option, end:
Option} on the rel/attribute row itself (types/src/records/rel.rs:91-98).
4. Promotion gate BEFORE any of this: winner must meet promotion_threshold (default 0.7) and min_evidence (default 1) (mod.rs:3577-3601, thresholds mod.rs:3054-3060); otherwise
flip to needs_review ("Promotion thresholds not met"), claims all Disputed, slot keeps a conflict_group link.
5. Group closed: resolved=true + rationale + resolved_at (mod.rs:3697-3722); TruthSlot upserted under deterministic id sha256(space::slot_key::"truth_slot") (mod.rs:1806-1821,
3754-3806) with current_values = [winner] (SingleWinner) / ranked list (ParallelValues) / sorted context winners, slot confidence = max of current values, stale cleared. Every
claim/group/slot write gets an AuditLog with before/after snapshots.
6. Events variant (`execute_tm_commit_events`, mod.rs:3118-3224): winner Approved; losers Rejected if confidence < EVENT_REJECT_FLOOR (env-overridable, default 0.2,
mod.rs:3226-3232) else Superseded; no validity windows.

NOTE — stub: `handle_temporal_personality` (mod.rs:2697-2729, StateOverTime "close open windows" logic) and `ValidityWindow` at mod.rs:2634-2692 are referenced ONLY by tests
(mod.rs:5102-5136); the production close path is exclusively apply_supersession_window/apply_winner_window above.

=====================================================================
5. WHEN THE LLM TIE-BREAK (AgenticSlotReasoner) FIRES vs DETERMINISTIC
=====================================================================

Gate: `should_use_reasoner` (mod.rs:2380-2395) requires ALL THREE: (a) claims.len() >= high_conflict_threshold (default 3), (b) any claim confidence >= 0.9 OR evidence_count >= 3,
(c) the DETERMINISTIC result is NeedsReview. Wrapper `resolve_slot_with_reasoner` (mod.rs:2397-2426) always computes the deterministic result first, returns it outright for
RequireMoreEvidence, and falls back to it on any reasoner error/None.

Reachability analysis (important, code-verified): in `execute_tm_resolve_slot`, AllowParallel is short-circuited before the reasoner (freshness-weighted ranking,
mod.rs:2140-2147); SupersedeByRecency returns NeedsReview only for an EMPTY claim set (mod.rs:2526-2529), which fails condition (a); SplitByContext never returns NeedsReview
(mod.rs:2533-2561); RequireMoreEvidence returns early. Therefore the agentic tie-break is WIRED BUT EFFECTIVELY UNREACHABLE in the production rel/attr path — deterministic
resolution always wins. The unit tests only exercise the skip paths with a FakeReasoner (mod.rs:4909-4988).

If it did fire: feature-gated `meerkat-extraction`, requires ANTHROPIC_API_KEY (silently returns None otherwise, mod.rs:2010-2013), model "claude-3-5-sonnet", 512 tokens, 10s
timeout (BoundedReasoningConfig defaults, mod.rs:1912-1923). Prompt lists claim_id|confidence|evidence_count|sources and demands JSON {"decision":"winner"|"needs_review",
winner_id, rationale} (mod.rs:1966-1997); winner_id is validated against the actual claim-id set; last-resort fallback substring-scans the raw response for any claim id
(mod.rs:2042-2081). It never sees claim VALUES — only ids, confidences, evidence counts and assertion-id "sources".

=====================================================================
6. END-TO-END LOOP (promotion → outbox → TM)
=====================================================================

Promotion never computes slot_key. `promote_assertions_inline` (crates/pipeline/src/scheduler/inline_promotion.rs:84-412) creates assertion + event + rel/attribute rows and
accumulates RecordRefs {id, table: "rel"/"attribute"/"event", op} into ONE OutboxPayload "inline_promotion.completed" (lines 395-409). The TM worker in
bin/hub-pipeline/src/main.rs [pseudonymous, redacted binary name; the line numbers are from the original source tree named in this answer's header] (~5990-6153) polls system_event rows per space by seq cursor (with seq-reset detection), maps records→TriggerEvents (`trigger_from_record_ref`,
mod.rs:3957-3972; slot_keys computed HERE at detection time from the fetched rows), calls `execute_tm_detect_conflicts_from_outbox_events`, then: executes each DirectCommit's
tm_commit INLINE (main.rs:6088-6102) and enqueues one `tm_resolve_slot` work item per conflict group (priority 50, idempotency key `tm_resolve_slot:{group_id}:{last_seq}`,
main.rs:6104-6137). A parallel work-queue lane exists: TM_DETECT_CONFLICTS_KIND items (payload = system_event ids) enqueue tm_resolve_slot + tm_commit items (main.rs:5283-5379);
TM_RESOLVE_SLOT_KIND loads the group, no-ops if already resolved, resolves via `resolve_conflict_group_with_storage` (re-fetching live claim rows for the group's claim_ids,
tolerating rows deleted since detection — mod.rs:2169-2291), and enqueues TM_COMMIT_KIND (main.rs:5398-5481); obsolete groups (all claims cleaned up) are marked resolved with
rationale "No active claims remain after cleanup" (main.rs:5435-5454).

=====================================================================
7. SUMMARY FOR MOBKIT (what the signal-only ledger lacks, precisely)
=====================================================================

- Detection is purely structural: same (subject, predicate[, object], security-envelope-hash) slot + >1 live claim rows ⇒ conflict. Zero value semantics: no normalization, no
equality check, no type-aware comparison; identical values conflict, different rel objects never do (cardinality unenforced).
- The slot_key is a reversible composite string, not a hash (except for events), enabling parse-back at resolution time; group and slot ids are deterministic hashes of the key so
detection is idempotent/upserting.
- supersede_by_recency = argmax over per-claim timestamps derived from evidence refs filtered by a per-predicate recency_basis with a declared fallback chain; ties break by
iteration order; confidence only matters afterward as a promotion gate (0.7/1-evidence default), which downgrades the whole slot to Disputed/needs-review rather than picking a
lesser winner.
- The loser is closed by writing valid_to = winner's recency timestamp onto the loser row's own validity_window (start backfilled from the loser's own timestamp),
monotone-tightening only; the winner's window start is backfilled and left open.
- The LLM tie-break exists, is bounded (3+ claims, high salience, deterministic NeedsReview, 512 tok/10s), but is presently unreachable given the deterministic resolvers' output
shapes — the memory hub in practice is 100% deterministic for rel/attr truth.


================ GAP: What happens to the Codex memory workspace when Phase-2 consolidation fails partway? The git baseline is reset only on success, so if the consolidation agent
crashes, times out, or its 90s lease expires after making partial edits to MEMORY.md/memory_summary.md, do those partial edits persist in the worktree and show up in the NEXT
run's git diff indistinguishable from user hand-edits (getting laundered into consolidated memory), or is there a rollback (git checkout/reset of the dirty tree)? And who enforces
the memory_summary.md `v1` first-line sentinel — deterministic code at read/inject time, or only the next consolidation agent?

ANSWER:
All evidence gathered. Final report follows.

FINDINGS: Codex Phase-2 consolidation failure semantics and v1 sentinel enforcement

== 1. No rollback exists anywhere. Partial edits persist in the worktree on every failure path. ==

`job::failed` is pure DB/metrics bookkeeping. Its entire body (codex-rs/memories/write/src/phase2.rs:253-279) is a metrics counter increment plus `mark_global_phase2_job_failed` /
`mark_global_phase2_job_failed_if_unowned` DB calls. It performs zero filesystem operations. Every failure branch in `run()` (steps 2-8: `failed_prepare_workspace` :69,
`failed_sandbox_policy` :83, `failed_load_stage1_outputs` :102, `failed_sync_workspace_inputs` :118, `failed_workspace_status` :133, `failed_workspace_diff_file` :161,
`failed_spawn_agent` :180) and every failure branch in the post-agent handler routes only through `job::failed`.

Post-agent completion handler (`agent::handle`, phase2.rs:360-450), exhaustive branch analysis:
- Agent finishes non-Completed (errored, or thread terminated before final status — `loop_agent` synthesizes `AgentStatus::Errored` at phase2.rs:481-483): the `else` at
phase2.rs:434-436 calls `job::failed(..., "failed_agent")`. No workspace touch.
- Heartbeat detects lost ownership mid-run (phase2.rs:497-504) or heartbeat DB error (:505-512): `loop_agent` breaks with `Errored` → same `failed_agent` path. No workspace touch.
- Agent Completed but ownership confirm fails (`still_owns_lock == false`, phase2.rs:389-414): the `if still_owns_lock` block (:415) is skipped entirely — no baseline reset AND no
cleanup. The completed agent's edits remain uncommitted relative to the old baseline.
- Agent Completed, owns lock, but `reset_memory_workspace_baseline` errors (phase2.rs:416-418): `failed_workspace_commit` — workspace stays dirty vs old baseline.
- `shutdown_consolidation_agent` (memories/write/src/runtime.rs:371-389) only removes the thread and calls `shutdown_and_wait` with a 10s timeout — no filesystem operations.

The baseline reset is a MARK, not a checkout. `reset_memory_workspace_baseline` (memories/write/src/workspace.rs:43-46) = remove `phase2_workspace_diff.md` +
`reset_git_repository`. `reset_git_repository` (codex-rs/git-utils/src/baseline.rs:69-102) deletes `.git/` entirely, re-runs `gix::init`, and commits the CURRENT worktree as a
fresh single-commit baseline (`commit_current_tree`, baseline.rs:137-155). The only git command that says "reset" is `read-tree --reset HEAD` (baseline.rs:157-160), which rewrites
the INDEX from HEAD, not the worktree. There is no `git checkout`, `restore`, or worktree-file rollback capability anywhere in the git-utils crate or the memories/write crate —
the primitive to roll back doesn't exist in this codebase.

Next run inherits the dirt. `prepare_memory_workspace` (workspace.rs:13-20) calls `ensure_git_baseline_repository` (baseline.rs:78-92), which explicitly PRESERVES a usable
existing `.git` (early-returns if `gix::open` + `head_file_entries` succeed). So after a crash, the pre-crash baseline survives, and step 6's `memory_workspace_diff` →
`diff_since_latest_init` (baseline.rs:105-120) diffs HEAD tree vs current directory contents. The crashed agent's half-written MEMORY.md/memory_summary.md edits appear as `M
MEMORY.md` / `M memory_summary.md` — structurally identical to user hand-edits (uncommitted worktree changes carry no authorship).

Laundering is not just possible, it is INSTRUCTED. The consolidation prompt template (memories/write/templates/memories/consolidation.md, "Incremental update and forgetting
mechanism" section, ~lines 163-166) says verbatim: "Every changes in `{{ phase2_workspace_diff_file }}` are authoritative and must propagated and consolidated. If a changes
appears to be randomly placed in the files, it is probably a user change and you shouldn't just drop it. Make sure to add it to the overall memories consolidation". So the next
consolidation agent is explicitly told to treat the crashed predecessor's partial edits as authoritative, probably-user changes and fold them into durable memory. The only
mitigating instruction is the v1-schema-reset rule (below), which catches only the special case where the crash left memory_summary.md without a `v1` first line.

Lease clarification: the lease is 3600s, not 90s — `JOB_LEASE_SECONDS = 3_600`, `JOB_HEARTBEAT_SECONDS = 90` (memories/write/src/lib.rs:105-107). 90s is the heartbeat interval
that renews the 1-hour lease. If the process dies, the lease expires after up to 1h and another process can claim; since the memory workspace is a single shared directory
(`memory_root(&config.codex_home)`, phase2.rs:53), a lease takeover while the original agent is still alive (e.g., heartbeat starvation) means two agents can write the same
worktree concurrently; the loser merely skips its own baseline reset (phase2.rs:389-414) — its edits still land on disk.

== 2. The v1 sentinel is enforced ONLY by the next consolidation agent (LLM prompt instruction). Deterministic code never validates it. ==

Read/inject path: `build_memory_tool_developer_instructions` (codex-rs/ext/memories/src/prompts.rs:27-51) reads `memories/memory_summary.md`, `.trim()`s it, truncates by token
limit, and the ONLY validation is `if memory_summary.is_empty() { return None }` (prompts.rs:41-43). No first-line check of any kind. Whatever bytes are in the file — including a
half-written summary from a crashed consolidation agent, or a summary missing the `v1` line entirely — are rendered into the read_path.md template and injected as a
developer-policy prompt fragment via `contribute_thread_context` (ext/memories/src/extension.rs:51-71, `PromptFragment::developer_policy`).

Grep confirms: there is no `"v1"` string or first-line-sentinel check in any Rust code under ext/memories/, memories/read/, memories/write/, or codex-api's memories endpoint.
Every occurrence of the `v1` sentinel lives in the prompt template memories/write/templates/memories/consolidation.md (lines 23, 136, 148, 189, 461-468, 764, 783, 854): "read the
existing summary ... only if its first line is exactly `v1`; otherwise treat the summary as schema-incompatible and regenerate the whole file from scratch" (:136-137), "if
`memory_summary.md` is missing, empty, or does not start with exactly `v1`, regenerate only `memory_summary.md` from scratch" (:147-148), plus a final self-check "verify
`memory_summary.md` still begins with exactly `v1`" (:854). All of these are instructions to the consolidation LLM; none is code.

Net answer:
(a) Partial edits from a crashed/timed-out/lease-lost consolidation persist in the worktree — there is no rollback mechanism anywhere (the git layer can only mark baselines, never
restore files) — and they surface in the next run's diff indistinguishable from user hand-edits, where the prompt explicitly directs the next agent to preserve and consolidate
them ("authoritative", "probably a user change ... you shouldn't just drop it").
(b) The `v1` sentinel is a purely LLM-enforced contract checked by the NEXT consolidation agent; the deterministic read/inject path performs no validation and will silently inject
a corrupt or partial memory_summary.md into every session's developer instructions (subject only to an empty-check and token truncation) until the next successful consolidation
happens to rewrite it.

Key files: `<local checkout>/cc/codex/codex-rs/memories/write/src/phase2.rs` (:253-279 failed = DB-only; :389-436 completion handler; :481-512 loop_agent failure synthesis),
`<local checkout>/cc/codex/codex-rs/memories/write/src/workspace.rs` (:13-20, :43-46), `<local checkout>/cc/codex/codex-rs/git-utils/src/baseline.rs` (:69-102 reset = fresh commit of
current tree; :78-92 ensure preserves .git; :157-160 index-only reset), `<local checkout>/cc/codex/codex-rs/ext/memories/src/prompts.rs` (:27-51 no sentinel check),
`<local checkout>/cc/codex/codex-rs/ext/memories/src/extension.rs` (:51-71 injection), `<local checkout>/cc/codex/codex-rs/memories/write/templates/memories/consolidation.md` (:136-148,
:163-166, :189, :854), `<local checkout>/cc/codex/codex-rs/memories/write/src/lib.rs` (:105-107 lease/heartbeat constants).


================ GAP: Can MobKit's MarkdownAgentMemoryStore ever update or supersede a memory, or do repeated/contradictory remembers only accumulate? memory_id embeds a
nanosecond timestamp + pid + seq, so re-remembering identical title+body yields a NEW id and the remember path only retains-then-appends by exact memory_id (agent_memory.rs:~571)
— confirm that (a) identical content saved twice produces two records, (b) a corrected fact and its stale predecessor both survive and can be co-injected by contextual recall
(sorted score-then-updated_at), and (c) the only content-level pressure is the newest-first 512-record/8MiB retention — i.e. contradiction resolution is entirely absent, not just
prompt-delegated.

ANSWER:
ANSWER: MobKit's MarkdownAgentMemoryStore can NEVER update or supersede a memory. There is no update/upsert path anywhere in the write chain; repeated and contradictory remembers
strictly accumulate, and the only removal mechanisms are explicit `forget` by exact memory_id and the newest-first 512-record/8MiB retention cap. Contradiction resolution is
entirely absent — not implemented anywhere, not even delegated to a prompt. All claims below verified in code; all paths under
`<local checkout>/meerkat-mobkit/.claude/worktrees/memory-system/`.

## (a) Identical content saved twice produces two records — CONFIRMED

1. **memory_id is never content-derived-only.** `new_memory_id` (meerkat-mobkit/src/identity_first/agent_memory.rs:1090-1099) formats
`mem-{now_ns()}-{pid:x}-{seq:x}-{fnv1a(title+body):016x}`. The FNV-1a content hash (`stable_suffix`, :1101-1108) is only a suffix; the nanosecond timestamp (`now_ns`, :1083-1088),
process id, and a process-global `AtomicU64` sequence counter (`NEXT_MEMORY_ID_SEQ`, :1091-1092) guarantee every call yields a distinct id even for byte-identical title+body.

2. **The write path only dedupes by exact memory_id.** `remember` (:220-260) validates/normalizes, stamps `created_at_ms == updated_at_ms == now_ms()` (:249-256), calls
`new_memory_id` (:251), then `append_markdown_record` (:552-582), whose entire "merge" logic is:
   - `records.retain(|existing| existing.memory_id != record.memory_id);` (:571)
   - `records.push(record.clone());` (:572)

   Since a fresh id can never collide with a persisted one, the `retain` at :571 is effectively dead for the remember path (it only matters as an idempotency guard for the raw
`append_markdown_record` helper, which tests call directly with fixed ids). There is no comparison of title, body, tags, or content hash against existing records anywhere — grep
for dedup/duplicate/update/upsert/supersede in agent_memory.rs returns nothing.

3. **The codebase asserts this behavior as intended.** Test `memory_ids_are_unique_for_identical_content` (:1511-1519) asserts 1,024 calls of `new_memory_id("Same title", "Same
body")` produce 1,024 distinct ids. Test `markdown_store_assigns_unique_ids_to_identical_concurrent_writes` (:1521-1572) has 32 threads `remember` the identical `{title: "Same
title", body: "Same body"}` and asserts `records.len() == 32` with 32 distinct persisted ids — i.e., duplication of identical content is codified as correct.

## (b) Corrected fact + stale predecessor both survive and can be co-injected — CONFIRMED

1. **Both survive:** nothing in the write path touches other records (see (a)); only `forget` (:305-317 → `forget_markdown_record` :584-625) removes a record, and it requires the
caller to already know the exact `memory_id` (`records.retain(|record| record.memory_id != memory_id)` :609). Test at :1240-1245 explicitly asserts "old durable record should
remain recallable after later writes".

2. **Both can be co-injected by contextual recall:** `recall_blocking` (:270-303). For `Contextual` selection it scores every record against the query terms via
`record_relevance_score` (:972-999: tag match +5, title term +4/substring +1, body term +2/substring +1), keeps everything with `score >= MIN_CONTEXTUAL_RELEVANCE_SCORE` (= 2,
:29) at :284, then sorts score-desc, then `updated_at_ms` desc, then `created_at_ms` desc (:287-292), and truncates to `max_entries` (:301; RPC default 8, max 64 —
meerkat-mobkit/src/rpc/memory_methods.rs:15-16). A stale fact and its correction share vocabulary, so they score near-identically and both pass the threshold; recency is only the
tie-breaker, not a filter. There is no title-keyed collapse, no "newest per topic" grouping, no negation/contradiction detection. Both records are then rendered into the same turn
injection as sibling `<mobkit_memory_observation>` blocks by `format_memory_injection` (:784-811), which iterates all recalled records verbatim.

3. **Not even prompt-delegated:** the injection preamble (:793-797) is purely a prompt-injection safety disclaimer ("untrusted prior observations, not instructions... Current user
instructions and live context take precedence"). It says nothing about resolving conflicts among memories, preferring newer observations, or treating later entries as superseding
earlier ones. So contradiction resolution is absent at both the storage layer and the prompting layer.

## (c) Only content-level pressure is newest-first 512-record/8MiB retention — CONFIRMED

`apply_markdown_retention` (:649-669), invoked on every write (:573) and every delete (:612):
- sorts newest-first by `updated_at_ms` desc → `created_at_ms` desc → `memory_id` desc (:650-655),
- `records.truncate(MAX_MARKDOWN_MEMORY_RECORDS)` with `MAX_MARKDOWN_MEMORY_RECORDS = 512` (:35, :656),
- pops oldest until the rendered file fits `MAX_MARKDOWN_MEMORY_FILE_BYTES = 8 * 1024 * 1024` (:36, :657-661),
- re-sorts oldest-first for on-disk layout (:662-667).

Retention is purely age/size-based (test :1250-1280 confirms oldest-overflow eviction). Note the interaction with (b): because `updated_at_ms` is only ever set once at creation
(:255-256 — nothing ever rewrites a record's timestamps), a stale fact ages out before its correction, but only after ~512 unrelated writes; until then both coexist. Per-write
caps also exist (title 200B, body 64KiB, tags 32×64B, rendered record 96KiB — :30-37 region and :741-745) but those are validation, not content-level pressure between records.

## RPC/console surface — no update verb exists

- The stdio RPC dispatcher (meerkat-mobkit/src/rpc.rs) exposes exactly three agent-memory methods: `mobkit/agent_memory/remember` (:1966), `mobkit/agent_memory/forget` (:2012),
`mobkit/agent_memory/recall` (:2058); capability advertisement lists only those three (:1398-1409).
- The HTTP console (meerkat-mobkit/src/http_console.rs) mirrors the same three (:4308, :4357 region, :4185-4190) and its permission map has only WRITE/DELETE/VIEW actions for them
(:1570-1572).
- The parameter layer (meerkat-mobkit/src/rpc/memory_methods.rs:160-176, :185-351) defines only Remember/Forget/Recall request types; the remember params (:185-242) accept no
`memory_id`, so a caller cannot even address an existing record to overwrite it. The runtime layer is a pure passthrough (meerkat-mobkit/src/identity_first/runtime.rs:466-485
`remember_agent_memory` → `provider.remember`; :487-506 `forget_agent_memory`), and the `AgentMemoryProvider` trait itself (agent_memory.rs:159-195) defines only
recall/remember/forget — no update method exists at the trait level for any provider to implement.
- The `AgentMemoryRecord`'s `updated_at_ms` field (:108) is therefore vestigial as an "update" signal: no code path ever sets it to anything other than the creation time.

The closest thing to "supersede" available to a client is a manual read-modify-write: `recall` → pick the stale `memory_id` → `forget` it → `remember` the correction. Nothing
performs this automatically.

Separate note (adjacent, non-Markdown-store): the older ledger/mob-level memory index RPC (`parse_memory_index_params`, memory_methods.rs:398-469) does have `conflict: bool` /
`conflict_reason` fields — that subsystem at least models the notion of a conflicting fact — but that is the entity/topic store index, entirely disjoint from
`MarkdownAgentMemoryStore`, which has no conflict concept at all.
