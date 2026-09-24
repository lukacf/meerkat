# Evidence group B

[Audit index](../AUDIT.md)

<a id="b01"></a>

### B01: Compaction demo can panic while printing an ordinary Unicode model response

**Severity:** medium. **Verdict:** confirmed. **Examples:** 013.

**Original proof:** python3 -c 'text="a"*199+"—"+"b"*20; raw=text.encode(); print("013:",len(raw),"bytes; slice end 200 is UTF-8 continuation:",raw[200]&192==128)' printed '013: 222 bytes; slice end 200 is UTF-8 continuation: True'. This is an exact byte-boundary witness, not execution of the Rust example: Rust's str indexing at that offset panics.

- [`examples/013-context-compaction-rs/main.rs:140-143`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/013-context-compaction-rs/main.rs#L140-L143) The response preview indexes a UTF-8 String at byte offset min(len, 200), without checking that 200 is a character boundary.

**Independent challenge:** The actual response-preview expression is byte-indexed and can panic on valid UTF-8. Independently compiled that exact expression with rustc and caught a real Rust panic for 199 ASCII bytes followed by an em dash. ASCII and a short Unicode string do not panic. This is not merely a Python byte-boundary prediction.

**Accepted correction:** Make only the preview truncation and ellipsis boundary Unicode-safe. Either a maximum-byte prefix rounded down to a character boundary or an explicitly character-counted preview is acceptable; no agent/provider changes are needed.

- [`examples/013-context-compaction-rs/main.rs:136-143`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/013-context-compaction-rs/main.rs#L136-L143) A successful run result is followed by &result.text[..result.text.len().min(200)]; byte length also controls the ellipsis.

**Implementation (fixed):** Preview truncation rounds its byte limit down to a UTF-8 boundary and appends an ellipsis only when truncated. The executable calls the tested helper.
Changed: `examples/013-context-compaction-rs/main.rs`.
- `python3 extraction of the actual response_preview function and its tests from main.rs -> rustc --edition=2024 --test - -o examples/013-context-compaction-rs/.preview-regression; run binary; remove binary`: **pass** One actual-function regression passed, covering empty/short Unicode, exact limit, long ASCII, a multibyte character straddling the limit, zero budget, and ellipsis placement. No duplicate implementation was tested.
- `./scripts/repo-cargo test -p meerkat --features jsonl-store,session-compaction --example 013-context-compaction`: **blocked** Parent exclusively owns shared Cargo execution; linked example test requested and not run by this agent.

**Independent fix review (fixed):** The actual compaction response preview rounds its byte limit down to a UTF-8 boundary and emits an ellipsis only when text is omitted.
- [`examples/013-context-compaction-rs/main.rs:140-143`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/013-context-compaction-rs/main.rs#L140-L143) Executable uses response_preview rather than direct arbitrary byte slicing.
- [`examples/013-context-compaction-rs/main.rs:193-221`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/013-context-compaction-rs/main.rs#L193-L221) floor_char_boundary implementation and empty/short/exact/long/multibyte-crossing/zero-limit tests.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** preview_respects_utf8_and_only_marks_truncated_text passed against the real helper.

<a id="b02"></a>

### B02: Compaction preservation and trigger guidance contradict the current compactor

**Severity:** medium. **Verdict:** confirmed. **Examples:** 013.

**Original proof:** Static comparison with DefaultCompactor::should_compact and rebuild_history_under_pressure proves both contradictions. Existing compactor regression tests were read, not executed; registered Rust testing is parent-owned.

- [`examples/013-context-compaction-rs/README.md:19-23`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/013-context-compaction-rs/README.md#L19-L23) Preservation rules promise that the N most recent message pairs are preserved.
- [`examples/013-context-compaction-rs/main.rs:162-173`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/013-context-compaction-rs/main.rs#L162-L173) Printed configuration describes a cumulative-input-token threshold and says recent_turn_budget is the number of turns to 'always preserve verbatim'.
- [`crates/meerkat-session/src/compactor.rs:237-283`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-session/src/compactor.rs#L237-L283) Triggers use last_input_tokens, current estimated history tokens, whole-request forecast, and request byte pressure, not the lifetime cumulative input-token counter.
- [`crates/meerkat-session/src/compactor.rs:390-429`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-session/src/compactor.rs#L390-L429) Retention is a maximum: min(recent_turn_budget, turn_starts.len()-1), then further reduced until the retained-byte budget fits. At least the oldest live turn is discarded even when fewer than N turns exist.
- [`crates/meerkat-session/src/compactor.rs:1197-1220`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-session/src/compactor.rs#L1197-L1220) The existing test test_rebuild_below_turn_budget_still_discards_oldest_turn explicitly pins the contrary behavior.

**Independent challenge:** Both the hard tail-preservation guarantee and lifetime-cumulative trigger wording contradict the implementation. The retention selector guarantees progress by removing at least one live turn and may retain even fewer under byte pressure. The trigger reads current/history/request pressure and the last provider count, not lifetime cumulative usage. The example's intent to show compaction is sound; its promises are not.

**Accepted correction:** Correct the example README, comments, and printed reference: recent_turn_budget is an upper bound subject to compaction progress and capacity constraints; the token trigger is current/last-request pressure rather than cumulative billed input tokens. Do not alter the compactor to honor stale documentation.

- [`examples/013-context-compaction-rs/README.md:19-23`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/013-context-compaction-rs/README.md#L19-L23) Promises preservation of the N most recent message pairs without qualifying it as a maximum.
- [`examples/013-context-compaction-rs/main.rs:61-63,162-173`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/013-context-compaction-rs/main.rs) Prints 'Recent turns preserved: 2', 'cumulative input tokens', and 'always preserve verbatim'.
- [`crates/meerkat-session/src/compactor.rs:237-308`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-session/src/compactor.rs#L237-L308) Triggers include last_input_tokens, estimated_history_tokens, whole-request effective-input forecast, and encoded/estimated request bytes; cadence has explicit capacity-recovery exceptions.
- [`crates/meerkat-session/src/compactor.rs:390-429`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-session/src/compactor.rs#L390-L429) Retain count is min(recent_turn_budget, live_turn_count-1), then decremented until retained bytes fit.
- [`crates/meerkat-session/src/compactor.rs:1197-1220`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-session/src/compactor.rs#L1197-L1220) Existing regression test specifically expects only the second of two turns retained with recent_turn_budget=4. Read as corroboration; not claimed executed.

**Implementation (fixed):** README, comments and printed configuration describe recent_turn_budget as an upper bound subject to progress/capacity; token pressure is current-context/last-request, not lifetime billing. System-row wording accounts for superseded keyed prompts.
Changed: `examples/013-context-compaction-rs/main.rs`, `examples/013-context-compaction-rs/README.md`.
- `Inspect crates/meerkat-session/src/compactor.rs:230-275,350-450 against the updated README and printed reference`: **pass** Verified last input/history/request-forecast triggers and min(turn budget, live turns minus one), followed by byte-pressure retention reduction. Compactor implementation unchanged.
- `./scripts/repo-cargo test -p meerkat-session --lib --features session-compaction compactor::tests`: **blocked** Existing compactor regressions remain intact; parent may run them. No live compaction was observed.

**Independent fix review (fixed):** README, runtime output and printed configuration correctly describe recent_turn_budget as a maximum subject to progress/capacity and token pressure as current context/last request. Latest keyed System versions and unkeyed Systems are distinguished from superseded keyed instructions.
- [`examples/013-context-compaction-rs/README.md:12-30`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/013-context-compaction-rs/README.md#L12-L30) No guaranteed N-turn preservation or cumulative billed-token trigger claim remains.
- [`examples/013-context-compaction-rs/main.rs:45-66`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/013-context-compaction-rs/main.rs#L45-L66) Low threshold may trigger, and retention output says up to 2.
- [`examples/013-context-compaction-rs/main.rs:159-180`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/013-context-compaction-rs/main.rs#L159-L180) Printed config repeats progress/capacity and current-pressure semantics.
- [`crates/meerkat-session/src/compactor.rs:230-268`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/meerkat-session/src/compactor.rs#L230-L268) Owner reads last_input_tokens, estimated history and request forecast, not cumulative billing.
- [`crates/meerkat-session/src/compactor.rs:392-435`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/meerkat-session/src/compactor.rs#L392-L435) Retention min(turn_count-1) and capacity loop establish the documented upper bound and latest-System behavior.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** 013 target compiled and its preview test passed. B02 is a static documentation-to-owner verification; this command did not execute compactor unit tests or prove a live conversation triggers compaction.

<a id="b03"></a>

### B03: Printed semantic-memory reference uses a removed API and the wrong storage root

**Severity:** low. **Verdict:** partial. **Examples:** 014.

**Original proof:** python3 -c 'from pathlib import Path; import re; trait=Path("crates/meerkat-core/src/memory.rs").read_text(); factory=Path("crates/meerkat/src/factory.rs").read_text(); print("014 index method exists:",bool(re.search(r"async fn index\s*\(",trait)),"index_scoped exists:","async fn index_scoped(" in trait,"factory root:","self.store_path.join(\"memory\")" in factory)' printed index=False, index_scoped=True, factory root=True. The runnable pre-seeding path itself uses the correct scoped API.

- [`examples/014-semantic-memory-rs/main.rs:188-215`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/014-semantic-memory-rs/main.rs#L188-L215) The printed architecture names MemoryStore::index() and says factory memory is created from .rkat/memory/. It does not qualify the factory steps as opt-in.
- [`crates/meerkat-core/src/memory.rs:789-814`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/memory.rs#L789-L814) The actual indexing contract is index_scoped(MemoryIndexRequest), delegating to index_scoped_batch; no index() method exists.
- [`crates/meerkat/src/factory.rs:7038-7058`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/factory.rs#L7038-L7058) Memory is only wired when effective_memory is true, and HnswMemoryStore opens self.store_path.join("memory").
- [`crates/meerkat/src/factory.rs:3300-3316`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/factory.rs#L3300-L3316) A newly created factory defaults enable_memory to false.

**Independent challenge:** Confirm the nonexistent MemoryStore::index() reference and incorrectly fixed .rkat/memory root. Narrow the separate opt-in criticism: the very same printed block already explicitly shows AgentFactory::new(store_path).memory(true), so the example does not entirely omit enablement. The numbered wiring explanation could be clearer, but missing enablement is not an independent defect. The runnable scoped indexing/search setup is correct and must remain intact.

**Accepted correction:** Update the printed indexing API to the scoped contract and describe <factory store_path>/memory rather than .rkat/memory. The existing .memory(true) example may be retained and the numbered steps optionally labeled 'when enabled'; no new runtime enablement or indexing redesign is warranted.

- [`examples/014-semantic-memory-rs/main.rs:101-116,127-130`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/014-semantic-memory-rs/main.rs) Actual pre-seeding constructs MemoryIndexRequest with the session owner, calls index_scoped, and scopes the search dispatcher to that same session; this is counterevidence against a runtime indexing defect.
- [`examples/014-semantic-memory-rs/main.rs:193-208`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/014-semantic-memory-rs/main.rs#L193-L208) Printed reference nevertheless calls MemoryStore::index() and hard-codes .rkat/memory/.
- [`examples/014-semantic-memory-rs/main.rs:210-211`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/014-semantic-memory-rs/main.rs#L210-L211) Counterevidence to the broad enablement claim: explicitly prints AgentFactory::new(store_path).memory(true).
- [`crates/meerkat-core/src/memory.rs:789-814`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/memory.rs#L789-L814) Current trait exposes index_scoped(MemoryIndexRequest), delegating to index_scoped_batch; it has no index method.
- [`crates/meerkat/src/factory.rs:1121-1132,7038-7058`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/factory.rs) Effective enablement resolves explicit override/factory enable plus backend eligibility; enabled memory opens self.store_path.join("memory").

**Implementation (fixed):** Printed architecture uses index_scoped(request), search(scope, query, limit), and <factory store_path>/memory. Existing .memory(true) enablement and actual same-session indexing/search wiring remain unchanged.
Changed: `examples/014-semantic-memory-rs/main.rs`, `examples/014-semantic-memory-rs/README.md`.
- `Python assertions over examples/014-semantic-memory-rs/main.rs printed reference; inspect MemoryStore trait and AgentFactory store_path.join("memory")`: **pass** Current scoped indexing/search signatures and factory-relative root verified; obsolete index() and fixed root removed from the printed reference.

**Independent fix review (fixed):** Printed architecture uses the scoped indexing/search contract and the factory-relative memory directory. Existing .memory(true) guidance and same-session owner wiring are preserved.
- [`examples/014-semantic-memory-rs/main.rs:77-80`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/014-semantic-memory-rs/main.rs#L77-L80) One session identity is created for the memory demo.
- [`examples/014-semantic-memory-rs/main.rs:118-149`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/014-semantic-memory-rs/main.rs#L118-L149) Index metadata/scope and MemorySearchDispatcher use the same session identity.
- [`examples/014-semantic-memory-rs/main.rs:212-238`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/014-semantic-memory-rs/main.rs#L212-L238) Reference uses scoped APIs and factory-relative storage; the same full body is called by production and tests.
- [`crates/meerkat-core/src/memory.rs:799-813`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/meerkat-core/src/memory.rs#L799-L813) Owning trait exposes scoped indexing, not removed index().
- [`crates/meerkat/src/factory.rs:7046-7047`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/meerkat/src/factory.rs#L7046-L7047) Factory derives memory directory from self.store_path.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** Independent actual_demo_recalls_all_indexed_facts_through_same_session_tool_scope passes: same main body, four real adapter requests across two turns, actual memory_search dispatch results containing all five facts, preserved history and cleanup. Scripted raw client, not HTTP/HNSW coverage.

<a id="b04"></a>

### B04: Persistence roundtrip silently succeeds when no session was persisted

**Severity:** medium. **Verdict:** confirmed. **Examples:** 015.

**Original proof:** Static failure path: an ordinary standalone save miss is logged and the turn returns successfully; list can be empty and load can return None; the if-let skips verification, and turn 2 still uses the original live agent. python3 -c 'from pathlib import Path; source=Path("examples/015-session-persistence-rs/main.rs").read_text(); state=Path("crates/meerkat-core/src/agent/state.rs").read_text(); print("015 missing load accepted:","if let Some(session) = loaded" in source,"best-effort save logs warning:","tracing::warn!(\"Failed to save session: {}\", e)" in state)' printed True for both. No filesystem fault injection or live example run was performed.

- [`examples/015-session-persistence-rs/main.rs:61-84`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/015-session-persistence-rs/main.rs#L61-L84) The example asserts automatic saving in a comment, accepts an empty session list, ignores load returning None, then successfully continues the original in-memory agent.
- [`crates/meerkat-core/src/agent/state.rs:660-675`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/state.rs#L660-L675) Ordinary standalone session saving explicitly has best-effort semantics and RunResult carries no persistence-success claim.
- [`crates/meerkat-core/src/agent/state.rs:736-748`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/state.rs#L736-L748) A failed store.save in the BestEffort branch is only a tracing warning; it does not fail the run.
- [`crates/meerkat-store/src/adapter.rs:25-33`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-store/src/adapter.rs#L25-L33) StoreAdapter forwards to SessionStore; it does not upgrade the core loop's best-effort policy.

**Independent challenge:** The demo does not establish its advertised save/load roundtrip: load(None) is accepted and the next answer comes from the original in-memory agent. A normal non-routing standalone run has best-effort saves, so successful inference is not a persistence acknowledgment. Narrow the operational implication to missing/unverified persistence: real load/list I/O errors already propagate through ?, and no actual JSONL failure was induced here. The core's best-effort policy is not itself alleged to be wrong.

**Accepted correction:** Require a loaded session and verify its ID plus expected transcript content as part of the example result. If adding an explicit required save, propagate its error and describe it honestly rather than calling automatic best-effort saving guaranteed. Keep continuation explicitly same-process; no process-restart implementation is required.

- [`examples/015-session-persistence-rs/main.rs:39-51,61-84`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/015-session-persistence-rs/main.rs) Uses StoreAdapter, accepts any list length and load(None), then calls run on the same agent rather than using loaded content.
- [`crates/meerkat/src/agent_builder.rs:273-304`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/agent_builder.rs#L273-L304) The provided AgentSessionStore is passed as the factory session_store_override; facade construction does not replace the store with a persistence-acknowledging session host.
- [`crates/meerkat/src/factory.rs:895-906,7139-7141`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/factory.rs) AgentBuildConfig defaults checkpointer=None and StandaloneEphemeral; a checkpointer is wired only when supplied.
- [`crates/meerkat-core/src/agent/state.rs:736-748,807-833`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/state.rs) Ordinary saves use BestEffort and warn/return Ok on store failure; Required applies when committing a staged permanent model-routing handoff, which this example does not request.
- [`crates/meerkat-store/src/adapter.rs:25-33`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-store/src/adapter.rs#L25-L33) Save/load calls are direct forwarding; the adapter introduces no stronger guarantee.
- [`crates/meerkat-store/src/jsonl.rs:585-600`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-store/src/jsonl.rs#L585-L600) A missing session file is explicitly Ok(None), while other file errors propagate.
- [`examples/015-session-persistence-rs/README.md:3-7`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/015-session-persistence-rs/README.md#L3-L7) README already excludes process recovery; no restarted-host requirement should be added.

**Implementation (fixed):** The demo explicitly saves through SessionStore with propagated failure, requires load(Some), and compares loaded identity plus complete transcript to the saved session. The second turn is honestly identified as original in-process continuation, not recovery.
Changed: `examples/015-session-persistence-rs/main.rs`, `examples/015-session-persistence-rs/README.md`.
- `./scripts/repo-cargo test -p meerkat --features jsonl-store --example 015-session-persistence`: **blocked** Added direct save_and_verify tests with real local JsonlStore roundtrip and faulty stores for missing/no-op save, explicit save failure, wrong identity, and stale transcript. Awaiting parent-owned Cargo execution.

**Independent fix review (fixed):** The example explicitly requires save success and a present loaded session with identical ID/transcript. It cannot substitute a successful in-memory second answer for persistence proof; that continuation is separately labeled same-process.
- [`examples/015-session-persistence-rs/main.rs:66-88`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/015-session-persistence-rs/main.rs#L66-L88) Verified save/readback is required before listing/continuation; no optional-load success path remains.
- [`examples/015-session-persistence-rs/main.rs:113-132`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/015-session-persistence-rs/main.rs#L113-L132) Actual helper propagates save/load/serialization errors, rejects None and ID/transcript mismatches.
- [`examples/015-session-persistence-rs/main.rs:134-259`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/015-session-persistence-rs/main.rs#L134-L259) Real JSONL roundtrip plus missing/no-op, save failure, wrong ID and stale transcript fixtures exercise helper. FaultyStore delegates revision-guarded deletion to its actual inner store.
- [`examples/015-session-persistence-rs/README.md:16-19`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/015-session-persistence-rs/README.md#L16-L19) Automatic saving is explicitly best-effort; second turn is not recovery.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** All 3 persistence tests passed. Successful roundtrip executes JsonlStore; failure fixtures use real trait dispatch but do not simulate an OS disk-full/permission fault or restart a process.

<a id="b05"></a>

### B05: Mob examples mistake lifecycle-event availability for member response completion

**Severity:** high. **Verdict:** confirmed. **Examples:** 017, 018, 019.

**Original proof:** python3 -c 'events=[(1,"MobCreated"),(2,"MemberSpawned")]; poll=lambda cursor,limit:[e for e in events if e[0]>cursor][:limit]; print("017/018/019: first response-wait exits on",poll(0,1)); print("019: after snapshot, no later event:",poll(poll(0,100)[-1][0],1))' prints MobCreated as the first response-wait result and [] after the snapshot. This models the exact read/assignment mechanics verified in the Rust store, not a live mob execution.

- [`examples/017-mob-coding-swarm-rs/main.rs:256-290`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/017-mob-coding-swarm-rs/main.rs#L256-L290) send() is followed by a 'Waiting for response' loop polling after cursor 0. Any historical event ends the wait; no response is observed before retire_all.
- [`examples/018-mob-research-team-rs/main.rs:283-319`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/018-mob-research-team-rs/main.rs#L283-L319) The same cursor-0 predicate treats prior mob creation/spawn events as the lead's response and then retires the members.
- [`examples/019-mob-pipeline-rs/main.rs:339-380`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/019-mob-pipeline-rs/main.rs#L339-L380) The lint wait also polls from 0. The test wait snapshots the latest lifecycle cursor after internal_turn and waits for unrelated later lifecycle events; reaching the 60-second deadline is silently accepted.
- [`crates/meerkat-mob/src/runtime/builder.rs:6922-6935`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-mob/src/runtime/builder.rs#L6922-L6935) MobCreated is appended during create(), before any member task.
- [`crates/meerkat-mob/src/store/in_memory.rs:3048-3080`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-mob/src/store/in_memory.rs#L3048-L3080) The first stored event gets cursor 1, and poll(0, 1) returns it because the filter is cursor > after_cursor.
- [`crates/meerkat-mob/src/runtime/handle.rs:10474-10530`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-mob/src/runtime/handle.rs#L10474-L10530) External send's SubmitWork ack mode is IngressAccepted. In contrast, internal_turn explicitly uses TurnCompleted; the pipeline's awaited internal_turn must not be misreported as merely fire-and-forget.
- [`crates/meerkat-mob/src/runtime/handle.rs:14372-14423`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-mob/src/runtime/handle.rs#L14372-L14423) start_turn exposes a tracked MemberTurnHandle with committed completion and an optional event stream; ordinary mob event polling is not a substitute.

**Independent challenge:** The lifecycle log is not a correlated response-completion channel. Verified the actual standalone path, not only its acknowledgment enum: admit_direct_session_turn returns after an admission notification and observes the turn in a separate task. Thus 017/018 have no completion wait before retirement. Their cursor-zero loop immediately sees the already-persisted MobCreated. In 019 the awaited internal turns already wait for completion, so its first polling loop is redundant and its second can idle to a silently accepted deadline. Do not claim that 019's test stage races ahead of unfinished lint or that premature retirement necessarily aborts every lead call.

**Accepted correction:** For 017/018 use tracked member-turn completion with a bounded observation timeout, propagate turn failure/timeout, and show the answer before cleanup. For 019 remove lifecycle waits around completed internal turns or switch to exact turn observation when displaying responses. Keep its documented manual-dispatch/topology scope; do not add a real pass/fail pipeline engine.

- [`examples/017-mob-coding-swarm-rs/main.rs:256-290`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/017-mob-coding-swarm-rs/main.rs#L256-L290) External send, zero-cursor poll, lifecycle labels, then retirement; no turn result is displayed.
- [`examples/018-mob-research-team-rs/main.rs:283-319`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/018-mob-research-team-rs/main.rs#L283-L319) Same external-send/lifecycle-poll pattern; old lifecycle activity or an expired deadline is accepted as the end of 'Waiting for response'.
- [`examples/019-mob-pipeline-rs/main.rs:327-380`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/019-mob-pipeline-rs/main.rs#L327-L380) Both internal_turn calls are awaited. The lint loop polls from zero; the test loop snapshots existing events after completion, then waits for an unrelated new event or deadline.
- [`crates/meerkat-mob/src/runtime/builder.rs:6922-6935`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-mob/src/runtime/builder.rs#L6922-L6935) create emits MobCreated before returning.
- [`crates/meerkat-mob/src/store/in_memory.rs:3048-3080`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-mob/src/store/in_memory.rs#L3048-L3080) Cursors start at 1; poll(0,1) selects the first historical event.
- [`crates/meerkat-mob/src/runtime/handle.rs:10484-10530`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-mob/src/runtime/handle.rs#L10484-L10530) External send submits IngressAccepted; internal_turn submits TurnCompleted.
- [`crates/meerkat-mob/src/runtime/provisioner.rs:4589-4641,11499-11581`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-mob/src/runtime/provisioner.rs) Standalone admission waits only for admitted_rx, spawns a separate terminal waiter, and returns Ok. admit_tracked_turn forwards a completion sender through the same direct-session path, so correlated completion is supported without converting the demos into durable hosts.
- [`crates/meerkat-mob/src/runtime/handle.rs:14372-14423`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-mob/src/runtime/handle.rs#L14372-L14423) start_turn exposes MemberTurnHandle with a completion receiver and optional event stream.

**Implementation (fixed):** The prior 019 fixed claim is retracted: reviewer1883 reproduced autonomous admission falsely reported as completed turns (review-actual-main-http-summary.json). The validated follow-up makes all four actual pipeline members TurnDriven, declares lint/test profiles externally addressable, removes automatic kickoff calls, reuses 017's exact start_turn_bounded/wait_bounded observer for each manual stage, prints both real answers before advancement/retirement, and retires members plus shuts down the mob after success, typed failure, or timeout. 017/018 changes are retained, not edited by this follow-up.
Changed: `examples/017-mob-coding-swarm-rs/main.rs`, `examples/017-mob-coding-swarm-rs/tracked_turn.rs`, `examples/017-mob-coding-swarm-rs/README.md`, `examples/018-mob-research-team-rs/main.rs`, `examples/018-mob-research-team-rs/README.md`, `examples/019-mob-pipeline-rs/main.rs`, `examples/019-mob-pipeline-rs/README.md`.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo build --locked -p meerkat -p meerkat-mob --example 019-mob-pipeline --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** Rebuilt the actual production main, not the test harness. Binary SHA256 b3be9c46704505cc7da9b75fb661690786d431a26736298d1f9a1bfd29f49197.
- `/opt/homebrew/opt/python@3.14/bin/python3.14 $AUDIT_ARTIFACTS/review-actual-main-http.py $HOME/Library/Caches/rust-workspaces/meerkat-7f595016ca/targets/v4/1.94.1-aarch64-apple-darwin-e408947bfd/luka-crnkovicfriis-abk-vigilant-winner-fd4ba9968e/debug/examples/019-mob-pipeline --example 019 --expect-endpoint-blocked`: **pass** Same reviewer harness and zero provider responses now produce actual main exit1, no timeout, four refused CONNECTs, typed BoundedTurnWaitError::RuntimeTerminated with FatalFailure/Failed, no 'Lint turn completed', no 'Test turn completed', no Stage2 dispatch, and no 'Demonstration complete'. Wrapper asserted these additional output invariants. Local-only refused transport; no external credentials or provider requests.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --example 014-semantic-memory --example 019-mob-pipeline --example 024-host-mode-event-mesh --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** 10 tests passed: 014 one full-body test; 024 two full-body/owner-cleanup tests; 019 three new actual-pipeline tests plus existing schema and three shared tracked-turn regressions. Actual019 requires four spawned/wired members, both exact answers in order, no completion while blocked, fatal failure in either stage with no subsequent dispatch, explicit timeout of permanently blocked provider, empty service after retirement, and supervisor shutdown. Scripted raw clients exercise the real provider adapter/AgentLlmClient, not external transport.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo clippy --locked -p meerkat -p meerkat-mob --example 014-semantic-memory --example 019-mob-pipeline --example 024-host-mode-event-mesh --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills -- -D warnings`: **pass** All three actual examples and their dependency closure pass targeted Clippy with warnings denied.
- `./scripts/repo-cargo test -p meerkat-mob --example 017-mob-coding-swarm --example 018-mob-research-team --example 019-mob-pipeline`: **blocked** Historical command not run by the original agent. Historical helper tests covered 017/018 only, not 019's autonomous runtime semantics. Superseded for 019 by the passing whole-pipeline command above.
- `Python source assertions over examples/019-mob-pipeline-rs/main.rs`: **pass** No tokio sleep or cursor-based post-completion lifecycle wait remains in the manual pipeline.

**Independent fix review (fixed):** Reopened 019 defect is now independently verified fixed. All four real pipeline members explicitly use TurnDriven; tracked lint/test submissions are authorized by explicit external_addressable profiles and await their exact bounded results. No automatic kickoff provider calls occur. Actual answers are printed before advancement/retirement, and errors/timeouts run retirement plus shutdown before preserving the original failure. Whole-body tests cover pending responses, both stage failures and permanently blocked timeout cleanup. Rebuilt actual main with zero provider responses now exits 1, not 0, without either completed claim or Stage2 dispatch. The earlier failed binary and mistaken source-only review remain documented as historical evidence.
- [`examples/017-mob-coding-swarm-rs/tracked_turn.rs:9-43`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/017-mob-coding-swarm-rs/tracked_turn.rs#L9-L43) One result spec is passed through start_turn_bounded/wait_bounded, with timeout over admission plus observation; actual bounded result/status is printed.
- [`examples/017-mob-coding-swarm-rs/main.rs:229-235`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/017-mob-coding-swarm-rs/main.rs#L229-L235) Lead uses the tracked-turn-compatible TurnDriven lane.
- [`examples/017-mob-coding-swarm-rs/main.rs:263-295`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/017-mob-coding-swarm-rs/main.rs#L263-L295) Report-before-cleanup order, error retirement attempt and original error preservation.
- [`examples/018-mob-research-team-rs/main.rs:233-241`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/018-mob-research-team-rs/main.rs#L233-L241) Research lead also selects TurnDriven.
- [`examples/018-mob-research-team-rs/main.rs:291-324`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/018-mob-research-team-rs/main.rs#L291-L324) Same helper and explicit failure/success cleanup ordering.
- [`examples/019-mob-pipeline-rs/main.rs:180-200`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/019-mob-pipeline-rs/main.rs#L180-L200) Actual lint/test profiles explicitly authorize host-submitted tracked turns.
- [`examples/019-mob-pipeline-rs/main.rs:282-301`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/019-mob-pipeline-rs/main.rs#L282-L301) All four pipeline members explicitly use TurnDriven and omit unnecessary initial kickoff calls.
- [`examples/019-mob-pipeline-rs/main.rs:333-397`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/019-mob-pipeline-rs/main.rs#L333-L397) Both stages use shared exact report_turn; answers precede completion output. Success/error/timeout paths retire and shut down the mob; original failure wins over cleanup failure.
- [`crates/meerkat-mob/src/runtime/handle.rs:2614-2644`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/meerkat-mob/src/runtime/handle.rs#L2614-L2644) wait_bounded consumes this handle's exact completion rather than state/history.
- [`crates/meerkat-mob/src/runtime/handle.rs:3590-3775`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/meerkat-mob/src/runtime/handle.rs#L3590-L3775) Canonical wait validates attribution and propagates runtime termination/cancellation/finalization/extraction failures.
- [`examples/019-mob-pipeline-rs/main.rs:539-665`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/019-mob-pipeline-rs/main.rs#L539-L665) Tests execute production run_pipeline with all four real members, assert both exact answers/order, hold responses pending, propagate typed failure in either stage and timeout without provider release, and read the actual service owner to verify no sessions remain.
- [`examples/017-mob-coding-swarm-rs/tracked_turn.rs:153-298`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/017-mob-coding-swarm-rs/tracked_turn.rs#L153-L298) Existing shared real-mob tests continue passing for 017/018 and now 019.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** Independent rerun: 019 seven tests pass, including all three whole-pipeline cases; 017/018 each retain their three passing shared regressions. Whole pipeline success invokes exactly two provider requests, not kickoff requests.
- `python3 $AUDIT_ARTIFACTS/review-actual-main-http.py $HOME/Library/Caches/rust-workspaces/meerkat-7f595016ca/targets/v4/1.94.1-aarch64-apple-darwin-e408947bfd/luka-crnkovicfriis-abk-vigilant-winner-fd4ba9968e/debug/examples/019-mob-pipeline --example 019 --expect-endpoint-blocked`: **pass** Independent rebuilt-binary verification: SHA256 b3be9c46704505cc7da9b75fb661690786d431a26736298d1f9a1bfd29f49197, exit1, zero provider responses, four locally denied CONNECTs, typed BoundedTurnWaitError RuntimeTerminated/FatalFailure/Failed. Additional assertions prohibit Lint/Test completed, Stage2, and Demonstration complete output. Earlier fdeb8a... exit0 reproduction is preserved in supplemental artifacts.

<a id="b06"></a>

### B06: Coding-swarm TOML instructs the model to call nonexistent operator tools

**Severity:** medium. **Verdict:** confirmed. **Examples:** 017.

**Original proof:** python3 -c 'from pathlib import Path; import re; tools=set(re.findall(r"const TOOL_\w+: &str = \"([^\"]+)\";",Path("crates/meerkat-mob/src/runtime/tools.rs").read_text())); print("017 missing advertised operators:",[n for n in ["mob.spawn","mob.retire","mob.wire","mob.status","mob.complete"] if n not in tools])' lists all five missing names. This affects reuse of the shipped TOML; main.rs currently uses its own shorter inline definition rather than this file.

- [`examples/017-mob-coding-swarm-rs/mob.toml:42-58`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/017-mob-coding-swarm-rs/mob.toml#L42-L58) The inline orchestrator skill names mob.spawn, mob.retire, mob.wire, mob.status, and mob.complete().
- [`crates/meerkat-mob/src/runtime/tools.rs:813-901`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-mob/src/runtime/tools.rs#L813-L901) Member-local operator definitions use the local catalog constants, with typed tool schemas.
- [`crates/meerkat-mob/src/runtime/tools.rs:1438-1449`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-mob/src/runtime/tools.rs#L1438-L1449) Actual names are spawn_member, spawn_many_members, retire_member, force_cancel_member, member_status, wire_members, unwire_members, list_members, and mob_* flow tools. There is no mob.complete local operator.

**Independent challenge:** Parsed the shipped TOML role text and independently compared its five advertised operator names to the actual tool_def catalog. None is advertised or routed under those names. This is a real reusable-template defect, not a failure in main.rs's separately authored inline definition.

**Accepted correction:** Correct only the reusable role instructions to current operator names, and replace the impossible completion call with reporting completion to the host/caller. Do not add aliases or a new completion tool to make obsolete prose executable.

- [`examples/017-mob-coding-swarm-rs/mob.toml:43-55`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/017-mob-coding-swarm-rs/mob.toml#L43-L55) Instructs calls to mob.spawn, mob.retire, mob.wire, mob.status, and mob.complete().
- [`crates/meerkat-mob/src/runtime/tools.rs:808-901,1438-1449`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-mob/src/runtime/tools.rs) Actual member-local definitions use spawn_member, retire_member, wire_members, member_status, and other concrete constants. There is no mob.complete operator.
- [`examples/017-mob-coding-swarm-rs/main.rs:31-69,209-218`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/017-mob-coding-swarm-rs/main.rs) Runnable mob uses CODING_SWARM_TOML, not the shipped mob.toml; its shorter inline orchestrator text does not contain the false names.

**Implementation (fixed):** Reusable mob.toml teaches spawn_member, retire_member, wire_members and member_status; completion is reported to the host/caller instead of invoking nonexistent mob.complete.
Changed: `examples/017-mob-coding-swarm-rs/mob.toml`, `examples/017-mob-coding-swarm-rs/tracked_turn.rs`.
- `Python tomllib parses mob.toml and resolves every explicitly taught operator against TOOL_* constants in crates/meerkat-mob/src/runtime/tools.rs`: **pass** All four names resolve; no mob.complete call remains.
- `./scripts/repo-cargo test -p meerkat-mob --example 017-mob-coding-swarm historical_events_cannot_complete_blocked_turn_and_answer_precedes_retirement`: **blocked** The real mob fixture also captures actual model-visible tools and compares names parsed from the reusable role instructions to that catalog. Awaiting parent Cargo.

**Independent fix review (fixed):** Reusable orchestrator instructions teach real member-local operator names and report completion to the caller instead of inventing mob.complete.
- [`examples/017-mob-coding-swarm-rs/mob.toml:43-55`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/017-mob-coding-swarm-rs/mob.toml#L43-L55) spawn_member, retire_member, wire_members and member_status replace obsolete mob.* calls; final step reports completion.
- [`examples/017-mob-coding-swarm-rs/tracked_turn.rs:185-207`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/017-mob-coding-swarm-rs/tracked_turn.rs#L185-L207) Parses the actual shipped role and checks every explicitly taught mob operator against the captured real model-facing tool list; rejects mob.complete.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** historical_events_cannot_complete_blocked_turn_and_answer_precedes_retirement also validates the actual template operators against the constructed member catalog; passed in both shared-helper targets.

<a id="b07"></a>

### B07: Published pipeline flow template references an absent join schema

**Severity:** low. **Verdict:** confirmed. **Examples:** 019.

**Original proof:** python3 -c 'from pathlib import Path; print("019 declared join schema exists:", Path("schemas/join.json").exists(), Path("examples/019-mob-pipeline-rs/schemas/join.json").exists())' prints False False. Full scoped inventory contains only main.rs and README.md.

- [`examples/019-mob-pipeline-rs/main.rs:89-97`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/019-mob-pipeline-rs/main.rs#L89-L97) The sample PIPELINE_TOML declares expected_schema_ref = "schemas/join.json".
- [`examples/019-mob-pipeline-rs/main.rs:136-138`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/019-mob-pipeline-rs/main.rs#L136-L138) This template is printed, not executed; the missing schema is a defect in the published flow reference, not a demonstrated failure of the runnable manual-dispatch path.
- [`crates/meerkat-mob/src/runtime/flow.rs:2524-2540`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-mob/src/runtime/flow.rs#L2524-L2540) A named schema ref is read as a filesystem path, and a missing file yields MobError::SchemaValidation.

**Independent challenge:** The printed reference flow has a real unresolved asset, but the runnable manual pipeline does not execute that flow. Parsed the actual PIPELINE_TOML, resolved its join schema name, and attempted the same relative-file read from the documented repository cwd: NotFound. Neither repository-relative nor example-relative schema exists.

**Accepted correction:** Make the displayed template self-contained with an inline schema, or ship a resolvable asset and document its cwd, or unmistakably mark the schema path as a user-supplied placeholder. Keep the claim limited to the reusable printed flow.

- [`examples/019-mob-pipeline-rs/main.rs:89-97,136-138,221`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/019-mob-pipeline-rs/main.rs) PIPELINE_TOML names schemas/join.json and is only printed. The live definition comes from the distinct pipeline_toml variable.
- [`crates/meerkat-mob/src/runtime/flow.rs:819-820,2524-2550`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-mob/src/runtime/flow.rs) Named output-schema references are filesystem reads, with failures converted to MobError::SchemaValidation; the loader does not magically resolve a packaged schema.
- [`examples/019-mob-pipeline-rs/README.md:23-26`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/019-mob-pipeline-rs/README.md#L23-L26) Explicitly states the declared FlowSpec is not invoked, countering any claim this missing file currently breaks the manual run.

**Implementation (fixed):** Printed pipeline template embeds a JSON join-output schema rather than referencing absent schemas/join.json; README documents its self-contained scope.
Changed: `examples/019-mob-pipeline-rs/main.rs`, `examples/019-mob-pipeline-rs/README.md`.
- `Python tomllib parses actual PIPELINE_TOML and json.loads parses every expected_schema_ref`: **pass** Join schema is inline, requires string summary, and needs no cwd-relative asset.
- `./scripts/repo-cargo test -p meerkat-mob --example 019-mob-pipeline`: **blocked** Added real MobDefinition parsing + jsonschema validation regression for accepted/rejected join output. Awaiting parent Cargo.

**Independent fix review (fixed):** The printed flow's join schema is self-contained inline JSON and parses into FlowSchemaRef::Inline; no cwd-dependent missing schema asset is required.
- [`examples/019-mob-pipeline-rs/main.rs:88-97`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/019-mob-pipeline-rs/main.rs#L88-L97) Join expected_schema_ref contains the object/summary schema inline.
- [`examples/019-mob-pipeline-rs/main.rs:667-679`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/019-mob-pipeline-rs/main.rs#L667-L679) Parses actual PIPELINE_TOML through MobDefinition, requires Inline, compiles JSON Schema and checks valid/invalid results.
- [`examples/019-mob-pipeline-rs/README.md:39-41`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/019-mob-pipeline-rs/README.md#L39-L41) Explicitly documents self-contained schema and manual-dispatch scope.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** printed_flow_schema_is_self_contained_and_validates_join_output passed. Does not claim the illustrative flow was executed.

<a id="b08"></a>

### B08: Composed-agent search tool advertises a maximum it never honors

**Severity:** medium. **Verdict:** confirmed. **Examples:** 025.

**Original proof:** python3 -c 'from pathlib import Path; s=Path("examples/025-full-stack-agent-rs/main.rs").read_text(); print("025 _limit field:","_limit: usize" in s,"used:","args._limit" in s,"fixed result count:","\"total\": 3" in s,"fixed ticket:","TICKET-1234" in s)' prints field=True, used=False, fixed result count=True. Static execution of the search_docs arm with {"query":"API","_limit":1} necessarily emits three entries; no Rust tool dispatch was executed.

- [`examples/025-full-stack-agent-rs/main.rs:37-48`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/025-full-stack-agent-rs/main.rs#L37-L48) SearchDocsArgs names its public schema property _limit and describes it as 'Maximum results to return'. There is no serde rename to limit.
- [`examples/025-full-stack-agent-rs/main.rs:65-69`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/025-full-stack-agent-rs/main.rs#L65-L69) The model-facing input schema is generated directly from SearchDocsArgs.
- [`examples/025-full-stack-agent-rs/main.rs:84-98`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/025-full-stack-agent-rs/main.rs#L84-L98) The parsed limit is never read. Every successful search returns the same three results and total=3, even for a requested maximum of 0 or 1.

**Independent challenge:** SearchDocsArgs advertises a maximum, yet the handler never reads it and always constructs exactly three results. The underscore is genuinely part of the public schema because the struct has no serde rename and schema_for derives directly; the underscore's style alone is not the defect. Limits 0 and 1 violate the promised maximum.

**Accepted correction:** Either implement the maximum-results contract, preferably with a clearly named limit schema property and specified total semantics, or remove the argument/promise altogether. Do not turn the fixture into a real documentation service.

- [`examples/025-full-stack-agent-rs/main.rs:37-48,65-69,84-98`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/025-full-stack-agent-rs/main.rs) _limit defaults to 5 and is described as a maximum, but only args.query is consumed and three fixed entries plus total=3 are returned.
- [`crates/meerkat-tools/src/schema.rs:13-17`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-tools/src/schema.rs#L13-L17) schema_for forwards the derived type to the core schema helper.
- [`crates/meerkat-core/src/schema.rs:91-106`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/schema.rs#L91-L106) Core uses schemars::schema_for!(T); its postprocessing only ensures object properties/required keys and does not rename _limit.

**Implementation (fixed):** SearchDocsArgs exposes limit (default five); actual fixture dispatch bounds results by limit, including zero, while total counts the three pre-limit fixture matches.
Changed: `examples/025-full-stack-agent-rs/main.rs`, `examples/025-full-stack-agent-rs/README.md`.
- `./scripts/repo-cargo test -p meerkat --features jsonl-store --example 025-full-stack-agent`: **blocked** Added real DomainTools dispatch tests for omitted limit, 0, 1 and 10, pre-limit total, and actual schema property names. Awaiting parent Cargo.

**Independent fix review (fixed):** limit is the public typed/schema property; actual fixture dispatch caps returned entries with min(limit, fixture_count), while total consistently reports all pre-limit fixture entries.
- [`examples/025-full-stack-agent-rs/main.rs:36-48`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/025-full-stack-agent-rs/main.rs#L36-L48) Optional limit defaults to 5 and explicitly documents zero and total semantics.
- [`examples/025-full-stack-agent-rs/main.rs:84-99`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/025-full-stack-agent-rs/main.rs#L84-L99) Real dispatcher slices fixture results using the parsed limit.
- [`examples/025-full-stack-agent-rs/main.rs:290-318`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/025-full-stack-agent-rs/main.rs#L290-L318) Direct dispatch tests omitted/0/1/10 and schema has limit, not _limit.
- [`examples/025-full-stack-agent-rs/README.md:7-12`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/025-full-stack-agent-rs/README.md#L7-L12) Fixture/default/cap/total contract is explicit.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** search_limits_bound_results_without_changing_total_matches and tool_catalog_discloses_fixtures_and_exposes_limit passed.

<a id="b09"></a>

### B09: Support-ticket fixture reports real creation without disclosing simulation

**Severity:** low. **Verdict:** confirmed. **Examples:** 025.

**Original proof:** Read the complete create_ticket dispatch arm: it parses arguments, constructs a literal JSON object, and returns ToolResult with is_error=false. The scoped source check printed fixed ticket=True. This finding is about disclosure of a deliberately mocked side effect, not a requirement to integrate a real issue tracker.

- [`examples/025-full-stack-agent-rs/main.rs:72-76`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/025-full-stack-agent-rs/main.rs#L72-L76) The model-facing tool description promises to 'Create a support ticket in the issue tracker'.
- [`examples/025-full-stack-agent-rs/main.rs:100-113`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/025-full-stack-agent-rs/main.rs#L100-L113) The handler performs no mutation or issue-tracker call; it returns a successful open ticket with fixed ID TICKET-1234 and fixed timestamp.
- [`examples/025-full-stack-agent-rs/main.rs:199-211`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/025-full-stack-agent-rs/main.rs#L199-L211) The user prompt asks for an engineering ticket and the final model response is printed without a mock/simulation qualification.
- [`examples/025-full-stack-agent-rs/README.md:3-14`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/025-full-stack-agent-rs/README.md#L3-L14) The feature summary describes two domain tools but does not say their results are fixtures or that no support ticket is created.

**Independent challenge:** The source deliberately returns a fabricated ticket, but that simulation is not disclosed to the model or user. The entire handler only parses input and constructs a successful JSON result; it performs no issue-tracker write. A source-only 'Simulated documentation search' comment is counterevidence that search was intended as a fixture, not a disclosure to its caller. A live model actually saying 'created' was not observed and is not required to prove the misleading tool contract.

**Accepted correction:** Explicitly label fixture search and simulated ticket creation in README, model-facing descriptions, and visible output/result data. Keep them offline-safe; do not add a real issue tracker or treat successful simulation as an execution error.

- [`examples/025-full-stack-agent-rs/main.rs:65-76`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/025-full-stack-agent-rs/main.rs#L65-L76) Unqualified model-facing descriptions say search internal documentation and create a support ticket in the issue tracker.
- [`examples/025-full-stack-agent-rs/main.rs:88-113`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/025-full-stack-agent-rs/main.rs#L88-L113) Search has a source-only simulation comment. Ticket result is a literal TICKET-1234, open status, fixed timestamp, with is_error=false and no mock/simulated marker.
- [`examples/025-full-stack-agent-rs/main.rs:199-212`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/025-full-stack-agent-rs/main.rs#L199-L212) Prompt requests engineering ticket creation if needed and prints the resulting model answer without qualification.
- [`examples/025-full-stack-agent-rs/README.md:3-13,42-44`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/025-full-stack-agent-rs/README.md) Describes domain tools and excludes other integrations, but does not disclose fixture results or that no external ticket is created.

**Implementation (fixed):** Offline search and ticket fixtures are disclosed in model-facing descriptions, system instructions, result data, terminal banner, architecture output, and README. Ticket data states simulated=true, external_mutation=false, and no issue tracker ticket created. No external integration added.
Changed: `examples/025-full-stack-agent-rs/main.rs`, `examples/025-full-stack-agent-rs/README.md`.
- `Python source/README assertions for terminal/model/result fixture disclosure`: **pass** Both terminal and README disclose no real issue tracker mutation; descriptions and result data explicitly mark offline simulation.
- `./scripts/repo-cargo test -p meerkat --features jsonl-store --example 025-full-stack-agent`: **blocked** Added deterministic actual ticket dispatch test asserting simulation metadata and model-visible fixture descriptions. Handler has no external write path. Awaiting parent Cargo.

**Independent fix review (fixed):** Both model and terminal reader are told these are offline fixtures. Ticket results explicitly say simulated=true and external_mutation=false, so success no longer masquerades as an actual issue-tracker write.
- [`examples/025-full-stack-agent-rs/main.rs:64-78`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/025-full-stack-agent-rs/main.rs#L64-L78) Model-facing descriptions disclose offline simulation and no real service mutation.
- [`examples/025-full-stack-agent-rs/main.rs:91-119`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/025-full-stack-agent-rs/main.rs#L91-L119) Search and ticket results carry simulation disclosure; complete ticket arm only parses and constructs a result.
- [`examples/025-full-stack-agent-rs/main.rs:162-168`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/025-full-stack-agent-rs/main.rs#L162-L168) System behavior explicitly prohibits claiming a real ticket was created.
- [`examples/025-full-stack-agent-rs/main.rs:199-203`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/025-full-stack-agent-rs/main.rs#L199-L203) Visible banner discloses fixtures before provider execution.
- [`examples/025-full-stack-agent-rs/main.rs:321-337`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/025-full-stack-agent-rs/main.rs#L321-L337) Real dispatch regression asserts simulated, external_mutation=false and no-issue-tracker notice.
- [`examples/025-full-stack-agent-rs/README.md:7-17`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/025-full-stack-agent-rs/README.md#L7-L17) Fixed ticket ID/timestamp, canned search and live-LLM prerequisite disclosed.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** Ticket direct-dispatch and tool catalog regressions passed; no external issue tracker configured or contacted.
