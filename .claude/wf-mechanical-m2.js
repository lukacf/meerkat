export const meta = {
  name: 'dogma-mechanical-m2',
  description: 'Dogma-357 mechanical fan-out wave M2: 5 disjoint single-crate lanes, write-only',
  phases: [{ title: 'Fix' }],
}

const RULES = `
You are a WRITE-ONLY implementation agent inside a Rust workspace (Meerkat / rkat).
Project doctrine (dogma): singular TYPED authority over stringly/Value/JSON; FAIL CLOSED on unknown (never fail-open);
NO back-compat shims (pre-1.0 — delete cleanly, no tombstones); NO panic!/unwrap()/expect() in non-test code (use
Result/? or explicit match); errors propagate as typed variants (never swallow a fault with .ok()/unwrap_or default);
match surrounding idiom/naming; add a focused #[test] (the row's "Gate"); newtypes parse-at-boundary fail-closed.

HARD CONSTRAINTS:
- Do NOT run cargo/make/git or any build/test command. Write code only — you cannot build.
- Edit ONLY files in your assigned lane. Do not touch meerkat-core, meerkat-contracts, or the facade unless told.
- No new Cargo.toml dependencies.
- Verify-first: if a row is already satisfied, report ALREADY_CLOSED with file:line evidence; do not change it.
- If the fix needs an out-of-lane crate, do the in-lane part and report the rest in 'blockers'; do NOT edit out-of-lane.
Return (StructuredOutput) per-row dispositions with concrete file paths + 1-2 line evidence.
`

const SCHEMA = {
  type: 'object', additionalProperties: false, required: ['lane', 'rows'],
  properties: {
    lane: { type: 'string' },
    rows: { type: 'array', items: {
      type: 'object', additionalProperties: false, required: ['row', 'disposition', 'files', 'summary'],
      properties: {
        row: { type: 'string' },
        disposition: { type: 'string', enum: ['FIXED', 'ALREADY_CLOSED', 'BLOCKED'] },
        files: { type: 'array', items: { type: 'string' } },
        summary: { type: 'string' },
        evidence: { type: 'string' },
        test_added: { type: 'string' },
      },
    } },
    blockers: { type: 'array', items: { type: 'string' } },
  },
}

const LANES = [
  {
    label: 'hooks',
    crate: 'meerkat-hooks (src/lib.rs) — do NOT edit meerkat-core/config.rs (report that part as a blocker)',
    spec: `
#289 In-process hook handler registry silently overwrites stable handler identity. Make duplicate stable HookInProcessHandlerId registration a typed conflict/revision event instead of a silent HashMap::insert overwrite at lib.rs:~217 (register_in_process_handler): either reject re-registration of an existing id with a typed error, OR bump/record the existing AtomicU64 revision (lib.rs:~204) tied to handler identity so behavior change is observable. Dispatch (resolves by id) then reads a canonical, revisioned handler authority. Gate: test that re-registering an already-registered in-process handler id returns a typed conflict OR produces an observable revision change (no silent overwrite).`,
  },
  {
    label: 'mcp-server',
    crate: 'meerkat-mcp-server (src/lib.rs)',
    spec: `
#133 MCP skills provenance fabricates source identity records. lib.rs:~96 skill_source_provenance fabricates a fresh SourceIdentityRecord with Embedded transport and mcp:{display_name} fingerprint from display data. Project the REAL SourceIdentityRecord from the SourceIdentityRegistry the skill runtime owns (the RPC skills handler already returns real provenance via SkillListResponse) instead of reconstructing it. If the registry is not reachable from this seam, report BLOCKED with what plumbing is needed. Gate: test that MCP skills list/inspect provenance equals the registry record (transport/fingerprint match the real source, not the synthetic mcp: form).

#169 MCP live reload stages partial operations without transaction. lib.rs:~2552 the no-server-name branch loops stage_reload per active server and returns on first Err, leaving earlier staged reloads in effect. Either stage all-or-nothing (validate all, then commit, rolling back staged ops on failure) OR return a typed partial-status enumerating per-server outcomes instead of a bare error. Gate: test that failure on the Nth server rolls back the batch OR returns explicit per-server staged status.`,
  },
  {
    label: 'web-runtime',
    crate: 'meerkat-web-runtime (src/lib.rs) — may call meerkat-mob-pack public APIs read-only',
    spec: `
#163 WASM mobpack initialization discards parsed mobpack/trust semantics. lib.rs:~1104-1131 (init_runtime): the local parse_mobpack (lib.rs:~647) extracts/parses but performs NO signature/digest/trust verification, and the result is bound to _parsed and discarded. Call meerkat_mob_pack::trust::verify_extracted_pack_trust (and validate) over the extracted files+digest with a TrustPolicy (Strict by default for browser), surface a typed err_js on rejection, and thread the verified ParsedMobpack into the runtime build (used for skills/system prompt) instead of discarding it. Gate: WASM Rust test that init_runtime with an unsigned mobpack under Strict policy returns Err(invalid_mobpack/untrusted); a trusted-signed pack succeeds; _parsed is consumed not dropped.

#60 WASM API-key bootstrap (near-closed — add test). lib.rs:~285-329 (populate_realm_from_api_keys) already sorts by catalog provider_priority() and builds via typed from_inline_api_keys + Provider::parse_strict. Confirm no remaining stringly realm decisions and ADD a test pinning the catalog-owned order: with only openai_api_key set, the default realm binding order equals provider_priority() and each backend's provider comes from Provider::parse_strict (an unrecognized key sorts last and does not invent a binding). If already fully clean, this is mostly a test-addition. Gate: that test.`,
  },
  {
    label: 'session',
    crate: 'meerkat-session (src/event_store.rs, src/projector.rs, src/persistent.rs)',
    spec: `
#71 FileEventStore sequence owner advances before event bytes are appended. Harden by advancing the durable sequence owner AFTER the log bytes are flushed+synced: move self.write_sequence_owner(session_id, last_seq) from event_store.rs:~242 to AFTER file.sync_all() (event_store.rs:~289), still inside the held append_lock+acquire_sequence_lock. Gate: test that injects a write/flush failure after sequence allocation and asserts the next append reuses the same first_seq (no gap) and last_seq == log tail. Existing append/read tests stay green.

#144 SessionProjector partial writes can duplicate derived event files. The CheckpointState::Missing branch (projector.rs:~248) calls project(from_seq=1) which APPENDS (replace_existing=false) to an existing events.jsonl, so a crash after events.jsonl write but before checkpoint write duplicates derived events on resume. Change the CheckpointState::Missing arm to call self.replay(...) (truncate-and-rebuild) exactly as the Invalid arm already does (~:249). replay() already removes derived files first. Gate: test that writes events.jsonl, deletes the checkpoint file, calls resume(), asserts events.jsonl has no duplicated lines. Existing projector resume tests green.

#265 EventStore schema version is inert. StoredEvent.schema_version is written (event_store.rs:~271) but never read. In read_from (event_store.rs:~305-311), compare stored.schema_version to EVENT_SCHEMA_VERSION and return a typed EventStoreError::SchemaVersionMismatch on drift (fail closed) rather than silently projecting an unknown schema. Gate: test that a StoredEvent line with schema_version != EVENT_SCHEMA_VERSION causes read_from to return a typed mismatch error, not a silent project.

#4 Machine-authorized archive compatibility projection (likely RETIRE/doc-only). Verify in persistent.rs:~5072-5095 that save_compatibility_projection_only is called only AFTER protocol.retire_session succeeds and that load/list read RuntimeState::Retired as primary authority. If so, add only a doc-comment declaring source-truth=machine + rebuild-trigger (do NOT add a second authority). Report ALREADY_CLOSED if the doc already states this. Gate: confirm ordering; no behavior change.`,
  },
  {
    label: 'mob',
    crate: 'meerkat-mob (src/runtime/handle.rs) — do NOT edit meerkat-contracts',
    spec: `
#328 mob/cancel_work public surface is wired before work tracking exists. CancelWork is hardcoded Err(MobError::WorkNotFound) (handle.rs:~1938-1942) while contracts/RPC/SDK/MCP advertise an ok success shape. Without building the full work-tracking ledger (too large for this lane), make advertise match deliver: return a typed MobError::WorkCancellationUnsupported (add the variant) rather than a phantom WorkNotFound, so the contract honestly reports the op is unsupported. If exposing "unsupported" requires a contracts-side error-code addition, do the mob-side typed variant and report the contracts mapping as a blocker. Gate: test that mob/cancel_work returns the typed Unsupported (not WorkNotFound) until a real ledger exists.`,
  },
]

phase('Fix')
const results = await parallel(
  LANES.map((lane) => () =>
    agent(
      `${RULES}\n\n=== YOUR LANE: ${lane.label} ===\nAssigned crate/files: ${lane.crate}\n\nRows (verbatim; verify-first):\n${lane.spec}`,
      { label: `m2:${lane.label}`, phase: 'Fix', schema: SCHEMA }
    )
  )
)
return results.filter(Boolean)
