export const meta = {
  name: 'folklore-resweep',
  description: 'Fresh comprehensive Rule-4 String/JSON/Option folklore sweep over the whole workspace; re-judge everything incl. prior refutations; surface NEW confirmed violations',
  phases: [
    { title: 'Map', detail: 'partitioned folklore candidate discovery' },
    { title: 'Verify', detail: 'classify + adversarial refuter panel per candidate' },
    { title: 'Synthesize', detail: 'cluster + dedup vs known + final list' },
  ],
}

// ---- Rule-4 doctrine the agents apply (verbatim test) ----
const DOGMA = `DOGMA Rule 4 — "Truth Is Typed, Identity Is Canonical" (String Oracle pattern).
A semantic distinction that MATTERS must be a TYPE. A VIOLATION = meaning carried by an untyped
representation AND re-derived into a DECISION in shared/runtime/surface logic. The untyped carriers:
- string prefixes / string splits that recover identity or role
- enum-name text matched via .as_str()/== to drive a branch (round-tripping a typed enum through text)
- provider-native field names used in SHARED (non-provider-crate) logic
- path / naming conventions ("name starts with X means reserved/system")
- JSON Value whose "kind"/"type" field carries the real meaning (Value["kind"] oracle)
- error-MESSAGE substring matching to classify an outcome
- Option<T> that hides ownership (absence has >1 distinct meaning, or a parallel bool encodes the missing 3rd state)
- clear_*/set_* bool pairs that collapse an Inherit/Set/Clear tri-state
- "convention says this string means completed/detached/trusted/unreachable/archived"
LEGITIMATE (NOT violations) — do not flag:
- tagged-serde enums (#[serde(tag=...)]) / parse-at-a-boundary INTO a type
- provider-native fields used strictly INSIDE the owning provider crate
- strings used purely as display text, log lines, or test data
- Option<T> whose absence has exactly ONE harmless meaning ("explicit-or-default")
- newtypes wrapping a string for identity (that IS the type)
- transport/wire method dispatch by name that fails CLOSED at the surface
- internal private registry round-trips that never cross a semantic boundary`

// ---- The 13 already-confirmed clusters + 4 contested: agents must NOT re-report confirmed; MUST re-judge contested ----
const KNOWN = `ALREADY-CONFIRMED clusters (DO NOT re-report these exact sites; they are being fixed separately):
- H1: meerkat-core/src/session.rs SessionLlmIdentityOverride + clear_provider_params/clear_auth_binding; session_recovery.rs; meerkat-runtime meerkat_machine_types.rs SessionLlmReconfigureRequest; meerkat/src/session_runtime/llm_reconfigure.rs; meerkat-rpc handlers/turn.rs+handlers/mob.rs+session_runtime.rs (clear_* bools / TurnOverrides); meerkat-contracts/src/wire/mob.rs MobTurnStartParams clear_*; DSL catalog/dsl/session_document.rs ClearAndSet*.
- H2: comms_name format!+split, mob_realm_id dot/colon, persisted_mob_binding, synthetic_parent_peer_added_fields, MemberCommsName (meerkat-mob/build.rs, runtime/actor.rs, runtime/builder.rs, runtime/provisioner.rs; meerkat-mob-mcp/lib.rs+agent_tools.rs).
- M1: SESSION_ARCHIVED_KEY / "session_archived" Value::Bool terminality (meerkat-session/src/persistent.rs).
- M2: is_durable_session_sync_unsupported message.contains(...) (meerkat-session/src/persistent.rs).
- M3: FLOW_SYSTEM_MEMBER_ID_PREFIX "__flow_system_" .starts_with (meerkat-mob runtime/mod.rs, run.rs, validate.rs, runtime/actor.rs).
- M4: SystemNoticeBlock::Comms kind:String + match kind.as_str() fail-open (meerkat-core/src/types.rs).
- M5: oauth_flow.rs backend_kind String inequality gate (meerkat-auth-core).
- M6: SessionMetadata.realm_id Option<String> + the Option<String> realm carrier family (meerkat-core session.rs, service/mod.rs, session_recovery.rs; meerkat factory.rs).
- M7: MeerkatRunInput/MeerkatResumeInput.model Option<String> + ResumeOverrideMask (meerkat-mcp-server/src/lib.rs).
- M8: step_dependency_modes/step_collection_policy_kinds .as_str()->match enum round-trip (meerkat-mob/src/run.rs).
- M9: effective_step_timeout Option<String> flow_deadline_timeout_reason (meerkat-mob/src/runtime/flow.rs).
- M10: peer_response_terminal:{..} source string + live.rs starts_with/split_once("Payload:") (meerkat-openai/src/live.rs; producer meerkat-runtime/src/input.rs context_key()).
- L1: realm_profile_store_explicit: bool tri-state (meerkat-mob-mcp/src/lib.rs).

CONTESTED (the prior audit refuted these as NON-violations; RE-JUDGE them from scratch — report is_violation true/false with reasoning):
- meerkat-openai/src/live.rs ~2974 (provider-internal NetworkTimeout parse)
- meerkat-mcp-server/src/lib.rs ~1767 (tools/call wire-method routing by name)
- meerkat-mob/src/runtime/edge_locks.rs ~46 (internal edge-lock registry round-trip)
- meerkat-core/src/connection.rs ~571 (Option<&AuthBindingRef> resolver default)`

const MAP_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['partition', 'candidates'],
  properties: {
    partition: { type: 'string' },
    candidates: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['file', 'line', 'pattern_kind', 'snippet', 'why_suspect', 'severity_guess'],
        properties: {
          file: { type: 'string' },
          line: { type: 'integer' },
          pattern_kind: { type: 'string', description: 'string_prefix|string_split|enum_name_text|provider_native|path_convention|value_kind_oracle|error_text|option_hides_ownership|clear_bool_tristate' },
          snippet: { type: 'string' },
          why_suspect: { type: 'string', description: 'what semantic decision is driven by the untyped carrier' },
          severity_guess: { type: 'string', description: 'high|medium|low' },
        },
      },
    },
  },
}

const VERDICT_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['file', 'line', 'is_violation', 'pattern_kind', 'severity', 'summary', 'typed_owner_exists', 'typed_owner', 'repair_sketch', 'wire_impact', 'generated_machine_impact', 'durable_metadata_impact', 'overlaps_known_cluster', 'refuter_consensus'],
  properties: {
    file: { type: 'string' },
    line: { type: 'integer' },
    is_violation: { type: 'boolean' },
    pattern_kind: { type: 'string' },
    severity: { type: 'string', description: 'high|medium|low' },
    summary: { type: 'string' },
    typed_owner_exists: { type: 'boolean' },
    typed_owner: { type: 'string', description: 'name+location of the canonical type to adopt, or "NEW: <proposed>" if none exists' },
    repair_sketch: { type: 'string' },
    wire_impact: { type: 'boolean' },
    generated_machine_impact: { type: 'boolean' },
    durable_metadata_impact: { type: 'boolean' },
    overlaps_known_cluster: { type: 'string', description: 'cluster id (H1..L1) if this is actually part of an already-confirmed cluster, else "none"' },
    refuter_consensus: { type: 'string', description: 'how many of 3 refuters FAILED to refute (e.g. "3/3 could not refute => real", "2/3", "0/3 => not a violation")' },
  },
}

phase('Map')

const PARTITIONS = [
  { name: 'core-session-service', scope: 'meerkat-core/src/ — focus: session.rs, session_recovery.rs, service/, session_store.rs, session_durable_config_authority.rs, pending_continuation.rs, turn_admission.rs, connection.rs' },
  { name: 'core-rest', scope: 'meerkat-core/src/ — focus: types.rs, agent*, state.rs, runtime_epoch.rs, handles.rs, peer_meta.rs, memory.rs, compact.rs, completion_feed.rs, tool_scope.rs, lifecycle/, error.rs, skills.rs, image_content.rs, realtime_transcript_revision.rs (EXCLUDE generated/ — it is machine-emitted)' },
  { name: 'runtime', scope: 'meerkat-runtime/src/ — meerkat_machine*, ops_lifecycle, input.rs, runtime_loop.rs, handles/, dispatch_control, recover_or_create_ops_state' },
  { name: 'session-store', scope: 'meerkat-session/src/ + meerkat-store/src/ — ephemeral.rs, persistent.rs, compactor.rs, event_store.rs, projector.rs, sqlite/jsonl/memory stores' },
  { name: 'mob-runtime', scope: 'meerkat-mob/src/runtime/ — actor.rs, flow.rs, flow_frame_engine.rs, builder.rs, provisioner.rs, run.rs, bridge*.rs, local_bridge.rs, supervisor_bridge.rs, edge_locks.rs, mod.rs' },
  { name: 'mob-core-and-mcp', scope: 'meerkat-mob/src/ top-level (build.rs, ids.rs, storage.rs, backend.rs, run.rs, validate.rs, store/) + meerkat-mob-mcp/src/ + meerkat-mob-pack/src/' },
  { name: 'contracts-capabilities-schema', scope: 'meerkat-contracts/src/ (wire/, emit.rs, error codes) + meerkat-capabilities/src/ + meerkat-machine-schema/src/ (catalog/dsl — but DSL clear_* is H1, skip that)' },
  { name: 'provider-clients', scope: 'meerkat-client/src/ + meerkat-anthropic/src/ + meerkat-gemini/src/ + meerkat-openai/src/ + meerkat-live/src/ (provider-native fields INSIDE provider crates are LEGITIMATE — only flag leakage into shared types or shared-logic decisions)' },
  { name: 'auth-llm-providers-models', scope: 'meerkat-auth-core/src/ + meerkat-llm-core/src/ + meerkat-providers/src/ + meerkat-models/src/ — backend_kind, auth_method, provider/model identity, capability detection, normalize_backend' },
  { name: 'rpc-rest-mcp', scope: 'meerkat-rpc/src/ + meerkat-rest/src/ + meerkat-mcp/src/ + meerkat-mcp-server/src/ — router/handlers, wire-shape adaptation, surface-owned semantics' },
  { name: 'cli-facade', scope: 'meerkat-cli/src/ + meerkat/src/ (facade: factory.rs, service_factory.rs, surface/, session_runtime/) — surface/facade-owned semantics, defaults fabricated at surface' },
  { name: 'schedule-workgraph-misc', scope: 'meerkat-schedule/src/ + meerkat-workgraph/src/ + meerkat-hooks/src/ + meerkat-skills/src/ + meerkat-memory/src/ + meerkat-tools/src/ + meerkat-comms/src/ + meerkat-web-runtime/src/ + meerkat-agent-build-authority/src/' },
]

const maps = await parallel(
  PARTITIONS.map((p) => () =>
    agent(
      `You are a Rule-4 folklore MAPPER. WORK IN THE CURRENT WORKING DIRECTORY (a git worktree). READ ONLY.

${DOGMA}

YOUR PARTITION: ${p.name}
SCOPE: ${p.scope}

${KNOWN}

TASK: Sweep your scope thoroughly with ripgrep + targeted reads. Find EVERY candidate site where an untyped carrier (string/Value/Option/bool) drives a SEMANTIC DECISION. Hunt for: .starts_with(/.split(/.splitn(/.contains( on identity or classification; .as_str() followed by match/== that reconstructs or branches on a typed enum's name; serde_json::Value field "kind"/"type"/"status" reads that branch; matches!(err, ...) on message text; Option<T> fields paired with a sibling bool or whose None has multiple meanings; clear_*/set_* bool pairs; provider-native field names (anthropic/openai/gemini/native) referenced in shared (non-provider) code; path/name-prefix conventions ("__", "mob.", role-from-name).

Be GENEROUS at the map stage (a later verify pass refutes false positives) BUT do not re-report the ALREADY-CONFIRMED cluster sites above. Report real CURRENT file:line with a quoted snippet. Aim for completeness over precision. Return MAP_SCHEMA.`,
      { label: `map:${p.name}`, phase: 'Map', schema: MAP_SCHEMA }
    )
  )
)

// Flatten + dedup by file:line (barrier already passed). Cap per-file to avoid pathological floods.
const seen = new Set()
const candidates = []
for (const m of maps.filter(Boolean)) {
  for (const c of (m.candidates || [])) {
    const key = `${c.file}:${c.line}`
    if (seen.has(key)) continue
    seen.add(key)
    candidates.push(c)
  }
}
// Always include the 4 contested for explicit re-judgment.
const CONTESTED = [
  { file: 'meerkat-openai/src/live.rs', line: 2974, pattern_kind: 'provider_native', snippet: 'contested: NetworkTimeout parse', why_suspect: 'prior-refuted; re-judge', severity_guess: 'low' },
  { file: 'meerkat-mcp-server/src/lib.rs', line: 1767, pattern_kind: 'enum_name_text', snippet: 'contested: tools/call routing by name', why_suspect: 'prior-refuted; re-judge', severity_guess: 'low' },
  { file: 'meerkat-mob/src/runtime/edge_locks.rs', line: 46, pattern_kind: 'enum_name_text', snippet: 'contested: edge-lock registry round-trip', why_suspect: 'prior-refuted; re-judge', severity_guess: 'low' },
  { file: 'meerkat-core/src/connection.rs', line: 571, pattern_kind: 'option_hides_ownership', snippet: 'contested: Option<&AuthBindingRef> resolver default', why_suspect: 'prior-refuted; re-judge', severity_guess: 'low' },
]
for (const c of CONTESTED) {
  const key = `${c.file}:${c.line}`
  if (!seen.has(key)) { seen.add(key); candidates.push(c) }
}

log(`Map complete: ${candidates.length} unique candidates to verify (incl. 4 contested)`)

phase('Verify')

const verdicts = await pipeline(
  candidates,
  // stage 1: independent classifier re-traces from real code, applies carve-outs
  (c) =>
    agent(
      `You are a Rule-4 folklore CLASSIFIER. WORK IN THE CURRENT WORKING DIRECTORY. READ ONLY.

${DOGMA}

CANDIDATE: ${c.file}:${c.line} [${c.pattern_kind}] — ${c.snippet}
WHY FLAGGED: ${c.why_suspect}

${KNOWN}

TASK: Re-trace from the CURRENT real code around this site (read the file, follow the producer/consumer). Decide:
- Is the untyped carrier actually driving a SEMANTIC DECISION in shared/runtime/surface logic? Or is it a LEGITIMATE carve-out (display text, provider-internal, tagged-serde, parse-at-boundary, single-meaning Option, newtype, fail-closed transport dispatch)?
- If this is actually part of an ALREADY-CONFIRMED cluster (H1..L1), set overlaps_known_cluster to that id and is_violation=false (it's covered elsewhere).
- Does a canonical typed owner already exist, or must one be designed?
Set is_violation = your honest verdict (true only if it is a genuine NEW Rule-4 violation not covered by a known cluster). Fill refuter_consensus="classifier-only (not yet paneled)" for now. Return VERDICT_SCHEMA.`,
      { label: `classify:${c.file.split('/').pop()}:${c.line}`, phase: 'Verify', schema: VERDICT_SCHEMA }
    ),
  // stage 2: only genuine suspects face a 3-lens adversarial refuter panel; confirmed at >=2 non-refutes
  async (verdict, c) => {
    if (!verdict || !verdict.is_violation || verdict.overlaps_known_cluster !== 'none') {
      return verdict // legitimate, or already-covered — no panel needed
    }
    const LENSES = [
      'BOUNDARY lens: argue this is a legitimate parse-at-a-boundary or display/transport use, NOT a shared-logic semantic re-derivation.',
      'OWNERSHIP lens: argue a typed owner is NOT required here because the absence/string has exactly one harmless meaning, or the owner legitimately lives elsewhere.',
      'CARVE-OUT lens: argue this fits a documented legitimate exception (provider-internal, tagged-serde, newtype, internal-private-registry, fail-closed dispatch).',
    ]
    const refutes = await parallel(
      LENSES.map((lens) => () =>
        agent(
          `You are an adversarial REFUTER. WORK IN THE CURRENT WORKING DIRECTORY. READ ONLY.
${DOGMA}
CLAIM TO REFUTE: "${c.file}:${c.line} is a genuine Rule-4 violation." Summary: ${verdict.summary}
${lens}
Re-read the real code. Try HARD to refute the claim using your lens. Default to refuted=true ONLY if you find a solid carve-out; if you cannot honestly refute it, refuted=false. Be rigorous, not contrarian.`,
          {
            label: `refute:${c.file.split('/').pop()}:${c.line}`,
            phase: 'Verify',
            schema: {
              type: 'object', additionalProperties: false,
              required: ['refuted', 'reason'],
              properties: { refuted: { type: 'boolean' }, reason: { type: 'string' } },
            },
          }
        )
      )
    )
    const valid = refutes.filter(Boolean)
    const nonRefutes = valid.filter((r) => !r.refuted).length
    return {
      ...verdict,
      is_violation: nonRefutes >= 2,
      refuter_consensus: `${nonRefutes}/${valid.length} could not refute`,
    }
  }
)

phase('Synthesize')

const confirmed = verdicts.filter(Boolean).filter((v) => v.is_violation && v.overlaps_known_cluster === 'none')
const contestedVerdicts = verdicts.filter(Boolean).filter((v) =>
  CONTESTED.some((cc) => cc.file === v.file && Math.abs(cc.line - v.line) <= 8)
)

const synthesis = await agent(
  `You are the SYNTHESIS agent for a fresh Rule-4 folklore re-sweep. Below are the CONFIRMED NEW violations (survived a 3-lens adversarial panel at >=2 non-refutes, and not part of the 13 already-known clusters), plus the re-judgments of the 4 previously-contested sites.

CONFIRMED NEW (JSON):
${JSON.stringify(confirmed, null, 1)}

CONTESTED RE-JUDGMENTS (JSON):
${JSON.stringify(contestedVerdicts, null, 1)}

TASK: Cluster the confirmed-new violations into root shapes (like the original audit's clusters). For each new cluster give: an id (N1, N2, ...), a one-line title, the member sites, the typed owner (existing or NEW), wire/generated-machine/durable impact, severity, and an ordered repair sketch. Separately state the verdict on each of the 4 contested sites (uphold-refutation vs overturn-to-violation, with reasoning). Note any partition that looked thin/under-swept. Return a thorough structured markdown report as your final text.`,
  { label: 'synthesize', phase: 'Synthesize' }
)

return {
  candidate_count: candidates.length,
  confirmed_new_count: confirmed.length,
  confirmed_new: confirmed,
  contested_rejudgments: contestedVerdicts,
  synthesis,
}
