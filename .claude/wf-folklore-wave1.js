export const meta = {
  name: 'folklore-wave1',
  description: 'Parallel write-only fan-out: crate-local folklore clusters in mob / mcp / workgraph (existing or crate-local typed owners)',
  phases: [{ title: 'Implement', detail: 'one agent per disjoint crate-lane, write-only' }],
}

const COMMON = `You are fixing Rule-4 String/JSON/Option folklore in the Meerkat repo. WORK IN THE CURRENT WORKING DIRECTORY (the main git worktree; it already contains the committed H1 work). You do NOT inherit prior context.

DOGMA Rule 4: a semantic distinction that drives a DECISION must be a TYPE, not carried by a string/enum-name-text/Value-kind/Option-hiding-ownership/clear-bool. Fix = adopt the canonical typed owner (or define a small crate-local typed owner) and DELETE the re-derivation. Parse-at-boundary-into-a-type, display/log strings, and provider-native-inside-provider-crate are legitimate (don't touch).

READ THESE IN-REPO SPEC FILES for the exact per-site detail (current file:line, typed owner, ordered repair):
- .claude/folklore-specs/resweep_synthesis.md  (clusters N1-N10 + singletons S1-S7, with ordered repairs)
- .claude/folklore-specs/resweep_new.json       (per-site: file/line/typed_owner/repair_sketch/impact)
- .claude/folklore-specs/clusters.json          (the 13 confirmed clusters M1-M10/L1/H1-H2 with current_sites + repair_steps)

CRITICAL RULES:
- WRITE-ONLY: make the source edits, but do NOT run cargo/build/test commands (no \`cargo\`, no \`./scripts/repo-cargo\`, no \`make\`). The team lead builds and corrects afterward. Use rust-analyzer-style reasoning + careful reading; you may grep/read freely.
- Stay STRICTLY within your assigned crate(s)/files below. Do NOT edit any file outside them (another agent owns the rest).
- Do NOT edit anything under \`*/generated/\` or \`specs/\` (machine-generated).
- No \`.unwrap()\`/\`.expect()\`/\`panic!\` in non-test library code (this repo forbids it).
- Preserve behavior exactly; only the representation changes. Update the crate's own tests/fixtures to match.
- If a fix genuinely requires editing a file OUTSIDE your lane (a cross-crate type you don't own), STOP that specific fix, leave the site untouched, and report it in 'deferred_cross_crate' — do NOT reach outside your lane.

Return a structured report.`

const SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['lane', 'sites_fixed', 'files_edited', 'new_types', 'deferred_cross_crate', 'notes'],
  properties: {
    lane: { type: 'string' },
    sites_fixed: { type: 'array', items: { type: 'string' }, description: 'cluster/finding id + file:line + 1-line of what changed' },
    files_edited: { type: 'array', items: { type: 'string' } },
    new_types: { type: 'array', items: { type: 'string' }, description: 'any new typed owners introduced, with their location' },
    deferred_cross_crate: { type: 'array', items: { type: 'string' }, description: 'sites left untouched because they need a type/file outside this lane' },
    notes: { type: 'string', description: 'risks, behavior-preservation concerns, anything the lead must verify' },
  },
}

phase('Implement')

const LANES = [
  {
    label: 'wave1:mob',
    text: `LANE = meerkat-mob crate ONLY (meerkat-mob/src/**). Fix these folklore items (read the spec files for exact current lines + repairs):
- M8 (clusters.json): meerkat-mob/src/run.rs step_dependency_modes()/step_collection_policy_kinds() — typed enum -> .as_str() -> match text -> reconstruct the SAME typed enum, with a dead _=>Err(Internal) arm. Replace with the direct typed converter that already exists ~220 lines above in the same file; delete the dead arm.
- N5 (resweep, run.rs:~4544): step_status_snapshot calls flow_run::StepRunStatus::as_str() then from_flow_run_status(text) to rebuild the identical StepRunStatus with a dead arm. Use the existing typed converter step_status_from_flow_run (runtime/flow.rs:~1957; make it pub(crate) if needed); delete the dead text-matcher. (Same root cause as M8 — sweep the siblings.)
- M9 (clusters.json): meerkat-mob/src/runtime/flow.rs effective_step_timeout returns Result<(Duration, Option<String>)> where the Option<String> conflates the lifecycle decision (Some=fail-flow-no-retry / None=retryable) + display text. Introduce a crate-local typed enum StepTimeoutOutcome (e.g. { Retryable, FlowDeadlineExceeded { reason: String } }) and thread it through both dispatch sites; separate the lifecycle decision from the display string.
- M3 (clusters.json): FLOW_SYSTEM_MEMBER_ID_PREFIX \"__flow_system_\" .starts_with()/== reserved-identity check at validate.rs:~150, runtime/actor.rs:~7500/~8341 (+ run.rs). Add a typed predicate AgentIdentity::is_system_reserved() (meerkat-mob/src/ids.rs) and route all admission/validation sites through it; keep the prefix constant private to ids.rs as the single authority.
- N10 (resweep): (a) runtime/actor.rs:~9643 installed_sides: &[&str] of \"local\"/\"peer\" re-classified via .contains(&\"local\") — introduce a crate-local WiringSides flag type (Local|Peer with has_local()/has_peer()/empty()) used by the rollback helpers + producers. (b) runtime/path.rs:~20 resolve_context_path splits a flow-reference on '.' and matches the leading segment (\"params\"/\"steps\"/\"loops\") — introduce a FlowRef { root: FlowRefRoot, json_path: Vec<String> } AST parsed once (FlowRef::parse), match on root, keep the deep walk_json for the genuinely-dynamic leaf; fail closed on parse error.
Update meerkat-mob's own tests. If runtime/flow.rs's step_status_from_flow_run lives in meerkat-mob it's in-lane; if it's elsewhere, defer.`,
  },
  {
    label: 'wave1:mcp',
    text: `LANE = meerkat-mcp-server crate AND meerkat-mcp crate ONLY (meerkat-mcp-server/src/**, meerkat-mcp/src/**). Fix:
- M7 (clusters.json): meerkat-mcp-server/src/lib.rs MeerkatRunInput.model / MeerkatResumeInput.model Option<String> re-derived twice (effective model via unwrap_or + ResumeOverrideMask.model: bool via is_some()). The audit's framing is partly wrong (model is never clearable). Adopt the typed ModelId owner where it improves typing and remove the redundant is_some() derivation per the spec; keep behavior identical. Minimal, correct.
- S4 (resweep, mcp-server/src/lib.rs:~1148): tri-state read-timeout policy (Default/Fixed/Infinite) carried as no_timeout: bool + timeout_ms: Option<u64>; consumers (~2533/~2651) reconstruct Option<Duration> by precedence. Replace the bool+option pair with ONE tagged-serde field using the existing ToolDispatchTimeoutPolicy (ops.rs:182) or a thin wire sibling; delete the precedence reconstruction. (wire — but adopt the EXISTING typed owner; if it requires a NEW contracts type, defer.)
- N9-mcp (resweep, meerkat-mcp/src/connection.rs:~421): auth_failure_suggests_oauth substring-matches \"401\"/\"403\"/\"Unauthorized\"/\"Forbidden\"/\"Auth required\" to drive interactive OAuth login. Plumb a typed connect-failure outcome through the transport seam: capture StatusCode typed at AuthChallengeRecorder::record / surface it on StreamableConnectError (enum ConnectFailureKind { Auth { challenge }, Other } or auth_status: Option<StatusCode>), then rewrite the predicate to auth_challenge.is_some() || auth_status.is_some(); delete the .contains() fallbacks. Keep within meerkat-mcp.
- N7-workgraph-tool (resweep, mcp-server/src/lib.rs:~2066): WorkGraphToolError.code: String collapsed from the 9-variant WorkGraphError, re-matched via error.code.as_str() to WORKGRAPH_TOOL_* constants with a silent _=>-32603. IF you can introduce WorkGraphToolErrorCode purely within meerkat-mcp-server (or it already exists), do it: build the typed code directly from WorkGraphError in map_error(), add jsonrpc_code(), delete the surface string match + fallthrough. IF it requires a NEW type in meerkat-contracts, DEFER it (report in deferred_cross_crate).
Update the crates' own tests.`,
  },
  {
    label: 'wave1:workgraph',
    text: `LANE = meerkat-workgraph crate ONLY (meerkat-workgraph/src/**). Fix the two findings in meerkat-workgraph/src/service.rs (resweep_new.json — service.rs:~1327 medium enum_name_text wire+durable; service.rs:~1398 HIGH string_oracle_enum_name_roundtrip durable). Read the spec for the exact typed owners (they EXIST per the panel — typed enums laundered through their own variant-name text). Replace the typed-enum -> .as_str()/text -> re-parse round-trips with direct typed matches / the existing converters; delete dead arms. If a fix needs a NEW type outside meerkat-workgraph, defer it. Update meerkat-workgraph's own tests.`,
  },
]

const reports = await parallel(
  LANES.map((l) => () =>
    agent(`${COMMON}\n\n${l.text}`, { label: l.label, phase: 'Implement', schema: SCHEMA })
  )
)

return { reports: reports.filter(Boolean) }
