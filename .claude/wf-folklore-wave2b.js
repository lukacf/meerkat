export const meta = {
  name: 'folklore-wave2b',
  description: 'Parallel write-only: provider-identity (N2/S6), contracts surface DTOs (N6/N7/N8), openai-live typed peer-terminal (M10/N5)',
  phases: [{ title: 'Implement', detail: 'three file-disjoint hub subsystems, write-only' }],
}

const COMMON = `You are fixing Rule-4 String/JSON/Option folklore in the Meerkat repo. WORK IN THE CURRENT WORKING DIRECTORY (main git worktree; H1+wave1+wave2a committed). No prior context inherited.

DOGMA Rule 4: a semantic distinction that drives a DECISION must be a TYPE, not a string/enum-name-text/Value["kind"]/path-convention/Option-hiding-ownership. Fix = adopt/define the typed owner and DELETE the re-derivation. Legitimate (don't touch): parse-at-boundary-into-type, display/log strings, provider-native fields used INSIDE a provider crate.

READ for exact per-site detail:
- .claude/folklore-specs/resweep_synthesis.md   (clusters N1-N10 + S1-S7, ordered repairs)
- .claude/folklore-specs/resweep_new.json        (per-site file/line/typed_owner/repair_sketch/impact)
- .claude/folklore-specs/clusters.json           (13 confirmed clusters incl. M10)

RULES:
- WRITE-ONLY: edit source; do NOT run cargo/build/test/make (the lead builds + corrects). Read/grep freely.
- Stay STRICTLY within your assigned files/crates. Do NOT edit anything outside them, nor */generated/ nor specs/.
- No .unwrap()/.expect()/panic! in non-test library code. Preserve behavior; only representation changes.
- A new wire/contracts type that crosses the SDK schema (regen-schemas) is fine to introduce — note it so the lead can regen. But if a fix needs a file OUTSIDE your lane, STOP that site and report it in deferred_cross_crate (the lead wires cross-crate consumers).
Return the structured report.`

const SCHEMA = {
  type: 'object', additionalProperties: false,
  required: ['lane', 'sites_fixed', 'files_edited', 'new_types', 'deferred_cross_crate', 'wire_impact', 'notes'],
  properties: {
    lane: { type: 'string' },
    sites_fixed: { type: 'array', items: { type: 'string' } },
    files_edited: { type: 'array', items: { type: 'string' } },
    new_types: { type: 'array', items: { type: 'string' } },
    deferred_cross_crate: { type: 'array', items: { type: 'string' }, description: 'every out-of-lane call site the lead must update, with file:line + the exact edit' },
    wire_impact: { type: 'boolean', description: 'true if a meerkat-contracts wire/schema type changed (lead must run make regen-schemas)' },
    notes: { type: 'string' },
  },
}

phase('Implement')

const LANES = [
  {
    label: 'wave2b:provider-identity',
    text: `LANE (cluster N2 + singleton S6). Files you may edit: meerkat-llm-core/src/types.rs, meerkat-llm-core/src/adapter.rs, meerkat-anthropic/src/**, meerkat-gemini/src/**, meerkat-openai/src/client.rs (the LlmClient::provider impl ONLY — NOT live.rs, another lane owns it), meerkat/src/factory.rs.
- N2 (resweep_synthesis.md cluster N2): LlmClient::provider() -> &'static str is the untyped identity outlier. Change LlmClient::provider() (and the delegating AgentLlmClient::provider() if applicable) to return meerkat_core::Provider. Each provider impl returns its enum variant. Rewrite meerkat-llm-core/src/adapter.rs:119 apply_generic_provider_overrides as match self.provider { Provider::Anthropic=>.., Provider::Gemini=>.. (fold the "gemini"|"google" alias into Provider::Gemini), Provider::OpenAI|SelfHosted|Other => tag } — explicit no-op arms, NOT a silent _ => tag fall-through. Drop the Provider::from_name(client.provider()) round-trip at meerkat/src/factory.rs:~2634/2637. Keep a separate provider_label()/as_str() for the legitimate error/log sites (adapter 195/342/351/383, AgentError::llm) — do NOT retype those display uses.
- S6 (resweep, meerkat/src/factory.rs:1047): delete the dead (provider=="gemini").then(|| executors.get("google")) hardcoded alias fallback (the map is never keyed by "google"); plain executors.get(provider) suffices, or key by typed Provider.
IMPORTANT: changing the trait return type ripples to EVERY client.provider() / agent.provider() caller. Fix the ones in your lane (llm-core, providers, factory). For callers OUTSIDE your lane (e.g. meerkat-runtime, meerkat-rpc, meerkat-client, meerkat-mob), list each in deferred_cross_crate with the exact .as_str()/Provider adaptation the lead must apply.`,
  },
  {
    label: 'wave2b:contracts-dtos',
    text: `LANE (clusters N6, N7-status member, N8). Files you may edit: meerkat-contracts/src/**, meerkat-mob-mcp/src/lib.rs (N6 consumer), meerkat-session/src/persistent.rs (N8 revision consumer), meerkat-schedule/src/tools.rs (N8 target consumer), sdks/web/src/mob.ts (N7 parse consumer). Do NOT touch mcp-server (another lane already handled N7-tool-error; the N7 mob-result-status member is contracts+web-sdk only).
- N6 (synthesis): WireActionArgs.action: String at meerkat-mob-mcp/src/lib.rs:3137 (+ guard :3218, dispatch :3500 match action.as_str()). Add WireMobWireAction { Wire, Unwire } (#[serde(rename_all="snake_case")]) to meerkat-contracts/src/wire/mob.rs (sibling to WireMobLifecycleAction). Retype WireActionArgs.action -> WireMobWireAction; replace action=="wire" with matches!(.., Wire); replace match action.as_str() with an exhaustive typed match (delete the catch-all). Update mob-mcp test fixtures (lib.rs:~4128/4145/4167). wire schema already declares enum:["wire","unwire"] so the surface is unchanged.
- N7-status (synthesis cluster N7, the mob-result DTO member): MobSnapshotResult.status / MobStatusResult.status / MobRespawnResult.status / MobAppendSystemContextResult.status collapse typed domains to String; MobSnapshotResult.members: Vec<Value> collapses typed MobMemberListEntry. Add WireMobLifecycleStatus (mirror of MobState) + WireMobRespawnOutcome (mirror of MobRespawnError::TopologyRestoreFailed vs success); replace the four status:String fields; map producers from the typed source; replace MobSnapshotResult.members: Vec<Value> with Vec<MobMemberListEntryWire>. Update sdks/web/src/mob.ts parseMobRespawnResult to validate the tagged enum.
- N8 (synthesis): (a) meerkat-session/src/persistent.rs:5451 query.revision=="current" sentinel — introduce RevisionSelector { Current, Specific(RevisionId) }; parse "current" once at the wire/surface seam (ReadSessionTranscriptRevisionParams in contracts), match the enum internally. (b) meerkat-contracts/src/request_lifecycle.rs:40 session_create_runs_immediately matches value["initial_turn"].as_str()=="deferred" on raw Value — define a deserializable WireInitialTurn { RunImmediately, Deferred } in contracts (or add Deserialize to the existing InitialTurn), deserialize a typed view, keep the same fail-open default; fold the divergent rpc InitialTurn copy if reachable. (c) meerkat-schedule/src/tools.rs:434 rewrite_current_session_target_object matches target["target_kind"]=="session" && target["type"]=="current_session" on raw Value — add SessionTargetBinding::CurrentSession { action } arm and deserialize into the typed enum instead of poking Value.
This lane likely changes meerkat-contracts wire types -> set wire_impact=true. List any out-of-lane consumers (rpc router for revision/initial_turn) in deferred_cross_crate.`,
  },
  {
    label: 'wave2b:openai-live',
    text: `LANE (cluster M10 + resweep N5-live). Files you may edit: meerkat-core/src/handles.rs, meerkat-core/src/session.rs (ONLY the PeerResponseTerminalFact-adjacent items if needed — be surgical, avoid SessionMetadata), meerkat-runtime/src/input.rs, meerkat-openai/src/live.rs, meerkat-openai/src/live_adapter.rs, meerkat-openai/src/live_orchestration.rs.
- M10 (clusters.json): the producer builds a fully-typed PeerResponseTerminalFact (meerkat-core/src/handles.rs:509) and projects it into SystemNoticeBlock::Comms (meerkat-runtime/src/input.rs:965-993), but then context_append_to_pending_system_context_append (input.rs:1020-1032) FLATTENS it to free-text + source=format!("peer_response_terminal:{route}:{corr}"); the OpenAI realtime consumer (meerkat-openai/src/live.rs:372) re-parses via starts_with + split_once("Payload:") + JSON re-parse. Carry the typed PeerResponseTerminalFact (or a typed apply-intent) through to the realtime adapter instead of string-encoding+re-parsing. Mirror the already-retired "runtime:steer:" precedent. Delete the live.rs string parse.
- N5-live (resweep, meerkat-openai/src/live.rs:3825): typed Provider -> as_str().to_string() at two producers (live.rs:~2809/2852, live_orchestration.rs:~145) compared by String != across the projection boundary to reject a provider swap (fails OPEN on drift). Retype LiveProjectionSnapshot.provider_id (live_adapter.rs:~622) and OpenAiRealtimeSession.current_provider_id (live.rs:~1269) to meerkat_core::Provider; make RefreshProviderSwap{from_provider,to_provider} carry Provider; the :3825 guard becomes Provider == Provider.
Report wire_impact if PeerResponseTerminalFact crosses the comms wire (it may already be typed in contracts). List out-of-lane consumers in deferred_cross_crate. NOTE: another lane (provider-identity) edits meerkat-openai/src/client.rs concurrently — do NOT touch client.rs.`,
  },
]

const reports = await parallel(
  LANES.map((l) => () => agent(`${COMMON}\n\n${l.text}`, { label: l.label, phase: 'Implement', schema: SCHEMA }))
)
return { reports: reports.filter(Boolean) }
