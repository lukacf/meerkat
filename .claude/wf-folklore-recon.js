export const meta = {
  name: 'folklore-recon',
  description: 'Re-verify all 14 String/JSON/Option folklore clusters against current code + investigate machine codegen path',
  phases: [
    { title: 'Infra', detail: 'machine codegen + wire regen path' },
    { title: 'Verify', detail: 'per-cluster ground-truth + repair spec' },
  ],
}

const SPEC_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: [
    'cluster_id', 'still_present', 'summary', 'current_sites', 'typed_owner',
    'files_to_change', 'wire_impact', 'generated_machine_impact', 'durable_metadata_impact',
    'repair_steps', 'risks', 'test_targets', 'confidence',
  ],
  properties: {
    cluster_id: { type: 'string' },
    still_present: { type: 'boolean', description: 'Does the violation still exist in current code? false if already fixed.' },
    summary: { type: 'string', description: '2-5 sentence ground truth of the CURRENT state at real line numbers.' },
    current_sites: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['file', 'line', 'snippet', 'role'],
        properties: {
          file: { type: 'string', description: 'repo-relative path' },
          line: { type: 'integer' },
          snippet: { type: 'string', description: 'the actual current line(s)' },
          role: { type: 'string', description: 'producer | consumer | reject-branch | type-def | wire | test-fixture | typed-owner' },
        },
      },
    },
    typed_owner: {
      type: 'object',
      additionalProperties: false,
      required: ['exists', 'name', 'location', 'notes'],
      properties: {
        exists: { type: 'boolean' },
        name: { type: 'string' },
        location: { type: 'string', description: 'file:line where the canonical typed owner lives' },
        notes: { type: 'string', description: 'how it should be adopted; or what NEW type must be designed if none exists' },
      },
    },
    files_to_change: { type: 'array', items: { type: 'string' }, description: 'every file that must be edited' },
    wire_impact: { type: 'boolean', description: 'touches meerkat-contracts wire types or any serde-on-the-wire shape => regen-schemas + version-parity required' },
    generated_machine_impact: { type: 'boolean', description: 'touches meerkat-core/src/generated/*.rs => must change the machine SPEC/DSL and regenerate, NEVER hand-edit generated files' },
    durable_metadata_impact: { type: 'boolean', description: 'changes durably-persisted SessionMetadata/store shape => back-read/migration consideration' },
    repair_steps: { type: 'array', items: { type: 'string' }, description: 'ordered, concrete, file:line-anchored repair steps a coding agent can follow literally' },
    risks: { type: 'array', items: { type: 'string' } },
    test_targets: { type: 'array', items: { type: 'string' }, description: 'crate(s) and/or test names to validate this cluster' },
    confidence: { type: 'string', description: 'high | medium | low — confidence the repair spec is complete and correct' },
  },
}

const CODEGEN_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['session_document_spec_location', 'regeneration_command', 'set_and_clear_spec_sites', 'wire_regen_workflow', 'version_parity_files', 'notes'],
  properties: {
    session_document_spec_location: { type: 'string', description: 'Where is the SOURCE spec/DSL that generates meerkat-core/src/generated/session_document.rs? file path(s).' },
    regeneration_command: { type: 'string', description: 'exact command(s) to regenerate generated/session_document.rs after editing the spec' },
    set_and_clear_spec_sites: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['file', 'line', 'snippet'],
        properties: { file: { type: 'string' }, line: { type: 'integer' }, snippet: { type: 'string' } },
      },
      description: 'where in the SPEC (not generated output) the SetAndClear/ClearAndSet reject logic and the clear_*-bool inputs originate',
    },
    wire_regen_workflow: { type: 'string', description: 'exact make targets / scripts for regen-schemas + verify-version-parity, and what they touch' },
    version_parity_files: { type: 'array', items: { type: 'string' }, description: 'files that must agree on version / be regenerated' },
    notes: { type: 'string', description: 'gotchas: macOS stack-overflow on protocol codegen, TLC implications, machine-authority constraints, etc.' },
  },
}

phase('Infra')
const infra = await agent(
  `You are investigating the Meerkat codegen + wire-contract infrastructure. WORK IN THE CURRENT WORKING DIRECTORY (a git worktree). READ ONLY — make no edits.

CONTEXT: A dogma cleanup will modify the generated state machine "SessionDocumentMachine". Its generated Rust lives at meerkat-core/src/generated/session_document.rs (DO NOT hand-edit generated files — that violates machine authority). It is emitted from a machine SPEC/DSL. There are crates: meerkat-machine-codegen, meerkat-machine-dsl, meerkat-machine-dsl-core, meerkat-machine-schema, meerkat-machine-derive, meerkat-machine-kernels.

The cleanup (H1) needs to: remove the structurally-representable "SetAndClear" / "ClearAndSet" illegal fourth state from the LLM-identity reconfigure inputs, replacing (Option<T>, clear_*: bool) pairs with a tri-state TurnMetadataOverride<T>{Set,Clear}. The generated SessionDocumentMachine currently has a duplicate reject branch for SetAndClear (search generated/session_document.rs for "SetAndClear", "ClearAndSet", "clear_" to find them).

YOUR TASKS:
1. Find the SOURCE spec/DSL that generates meerkat-core/src/generated/session_document.rs. Look for *.tla, *.rs DSL definitions, build.rs, machine definition macros, or a codegen entrypoint. Trace from the generated file's header comment (it usually names its source) and from meerkat-machine-codegen.
2. Find the EXACT command to regenerate generated/session_document.rs (look in Makefile, xtask, build.rs, scripts/, .cargo/config.toml aliases). Note any macOS gotchas (e.g. 'ulimit -s 65520' for protocol-codegen stack overflow).
3. Locate, in the SPEC (not the generated output), where the SetAndClear/ClearAndSet reject logic and the clear_* bool inputs for LLM identity reconfigure originate.
4. Document the wire-schema regeneration workflow: 'make regen-schemas', 'make verify-version-parity' — what scripts they run, what they touch (artifacts/schemas/, sdks/*/generated/), and the version-parity file set.
5. List the version-parity files.

Return the structured CODEGEN_SCHEMA. Be exact with file paths and line numbers. Quote real snippets.`,
  { label: 'infra:codegen+wire', phase: 'Infra', schema: CODEGEN_SCHEMA }
)

phase('Verify')

const CLUSTERS = [
  {
    id: 'H1',
    label: 'H1:tristate-llm-identity',
    text: `H1 (HIGH) — Inherit/Disable/Set tri-state collapsed into Option<T> + clear_*: bool (LLM identity).
The (Option<T>, clear_*: bool) split is hand-rolled at 5+ seams: SessionLlmIdentityOverride (meerkat-core/src/session.rs ~3751), SessionLlmReconfigureRequest, StartTurnParams, MobTurnStartParams, SurfaceSessionRecoveryOverrides (meerkat-core/src/session_recovery.rs ~29,31). Meaning is re-derived via "if clear { None } else { x.or(current) }" (session_recovery.rs ~487,547). The illegal set+clear fourth state is structurally representable, forcing hand-maintained rejections (SessionLlmIdentityOverrideError::SetAndClear* session.rs ~3765) AND a duplicate reject branch in the generated SessionDocumentMachine (meerkat-core/src/generated/session_document.rs ~163-164,595-596). The canonical typed owner TurnMetadataOverride<T>{Set,Clear} already exists (meerkat-core/src/lifecycle/run_primitive.rs ~1262) and is correctly used by the parallel RuntimeTurnMetadata path; sibling ToolCategoryOverride{Inherit,Enable,Disable} (session.rs ~3919) already cites the dogma.
Proposed repair: replace every (Option<T>, clear_*) pair with Option<TurnMetadataOverride<T>> (None=inherit, Some(Set)=set, Some(Clear)=disable); deserialize legacy clear_* wire once at the serde boundary; delete SetAndClear*/ClearAndSet* runtime AND generated-machine reject branches; collapse the four "if clear {..}" sites into one .resolve(current).
NOTE: This threads through generated machine inputs across four surfaces (rpc, mob, facade, core). SessionLlmReconfigureRequest may be a wire type. ENUMERATE every clear_*/Option pair and every consumer. Identify exactly which fields participate (model, provider, provider_params, auth_binding, etc.). Map which structs are wire (meerkat-contracts) vs domain.`,
  },
  {
    id: 'H2',
    label: 'H2:mob-identity-from-string',
    text: `H2 (HIGH) — Mob member/realm identity reconstructed from comms_name path-split + realm format-string.
comms_name="{mob_id}/{profile}/{agent_identity}" is built from three typed values (meerkat-mob/src/build.rs ~145) then re-split in persisted_mob_binding (meerkat-mob-mcp/src/lib.rs ~185-206) and synthetic_parent_peer_added_fields (meerkat-mob-mcp/src/agent_tools.rs ~123) to recover MobId/role/AgentIdentity, driving owns_persisted_bridge_session ownership/routing after restart and outbound mob.peer_added payloads.
CONFIRMED LATENT BUG to verify: producer mob_realm_id emits "mob.{id}" (dot) at build.rs ~24 (the only form RealmId::parse allows; slug rejects ':'), while the consumer matches "mob:{id}" (colon) at lib.rs ~195 — so the path NEVER matches in production; only hand-fabricated test fixtures (lib.rs ~5384/5426) keep it green. The peer_meta.labels cross-check is a stringly HashMap<String,String> by magic keys. The agent_tools fallback FAILS OPEN, inventing role="external".
Proposed repair: persist a typed MobMemberBinding{ mob_id: MobId, role: ProfileName, member: AgentIdentity } as a first-class SessionMetadata field at build time; read it directly. Introduce a MemberCommsName newtype (single Display + fail-closed FromStr) to replace all format!+split sites. Derive mob membership via ONE shared mob_realm_id helper used by BOTH producer and consumer (kills dot/colon divergence at compile time). Model unknown parent role as typed PeerRole::External, not a synthetic string.
VERIFY the dot/colon bug literally. Enumerate every format! + split site for comms_name. Confirm current SessionMetadata shape and whether comms_name is already persisted there. Confirm ProfileName/AgentIdentity/MobId typed owners exist.`,
  },
  {
    id: 'M1',
    label: 'M1:archived-from-value-bool',
    text: `M1 (MEDIUM) — Archived/terminal lifecycle from Value::Bool in standalone session path.
A SESSION_ARCHIVED_KEY bool decides terminality in the runtime-LESS branch (meerkat-session/src/persistent.rs ~1448). The runtime-backed path correctly owns it via RuntimeState::Retired. Verify the standalone path re-derives terminality from a stored JSON bool rather than the canonical machine-owned terminal state. Find the canonical owner (RuntimeState::Retired or a SessionDocumentMachine terminal state). Propose how the standalone path should obtain terminality canonically.`,
  },
  {
    id: 'M2',
    label: 'M2:error-text-classification',
    text: `M2 (MEDIUM) — Capability-unsupported classified by error-message substring.
is_durable_session_sync_unsupported does message.contains("durable session snapshot synchronization is not supported") (meerkat-session/src/persistent.rs ~253) to steer discard-vs-error. The SessionError::Unsupported idiom exists but is bypassed. Verify the substring match, find SessionError::Unsupported (and its error_code), and propose replacing the text match with a typed error-variant check.`,
  },
  {
    id: 'M3',
    label: 'M3:flow-reserved-prefix',
    text: `M3 (MEDIUM) — Flow reserved-identity namespace by AgentIdentity string prefix.
FLOW_SYSTEM_MEMBER_ID_PREFIX="__flow_system_" is re-derived via .starts_with() at 3 admission sites (meerkat-mob/src/runtime/validate.rs ~147, meerkat-mob/src/runtime/actor.rs ~7500,8341). Verify all .starts_with(FLOW_SYSTEM_MEMBER_ID_PREFIX) sites. Propose a typed predicate (e.g. AgentIdentity::is_system_reserved() or a typed MemberKind) so reserved-ness is a method/type, not a scattered string prefix check. Confirm where the prefix constant is defined and where identities are minted.`,
  },
  {
    id: 'M4',
    label: 'M4:systemnotice-comms-kind',
    text: `M4 (MEDIUM) — SystemNoticeBlock::Comms.kind: String re-matched in core projection.
Typed PeerConvention/InputKind are flattened to kind:String (meerkat-core/src/types.rs ~1169), then re-derived via "match kind.as_str()" with a fail-open "_=>" arm (types.rs ~1489). Literal string producers exist in 5 surface crates. Verify the kind:String field, the match kind.as_str() consumer + fail-open arm, and enumerate all producers across crates. Find the typed PeerConvention/InputKind owners. Propose replacing kind:String with the typed enum (parse once at boundary; remove fail-open). Flag wire impact if SystemNoticeBlock crosses the wire.`,
  },
  {
    id: 'M5',
    label: 'M5:oauth-backend-kind-string',
    text: `M5 (MEDIUM) — OAuth backend_kind validated by enum-name string equality.
backend_profile.backend_kind != expected_backend_kind (meerkat-auth-core/src/oauth_flow.rs ~368, both String/&'static str) gates auth-binding. Canonical NormalizedBackendKind + normalize_backend exist (audit cited meerkat-llm-core/.../binding.rs ~30, catalog.rs ~157) but are bypassed. Verify the string-equality gate, locate NormalizedBackendKind + normalize_backend in their CURRENT crate (may be meerkat-llm-core or meerkat-models or meerkat-providers), and propose comparing normalized typed kinds instead of raw strings. This is auth/credential-sensitive — be precise.`,
  },
  {
    id: 'M6',
    label: 'M6:realm-parallel-option-string',
    text: `M6 (MEDIUM) — Realm carried as parallel untyped Option<String> beside typed RealmId.
SessionMetadata.realm_id: Option<String> (meerkat-core/src/session.rs ~3693) parsed to RealmId only at the bottom resolution hop (meerkat/src/factory.rs ~1450); realm is already carried typed in auth_binding.realm. Verify the Option<String> field and where it's parsed to RealmId. Determine whether realm_id duplicates auth_binding.realm or is independent. Propose either typing the field as Option<RealmId> (parse once at serde boundary) or removing the duplicate if auth_binding.realm is canonical. Flag durable-metadata + wire impact (SessionMetadata persistence).`,
  },
  {
    id: 'M7',
    label: 'M7:mcp-model-option-mask',
    text: `M7 (MEDIUM) — MCP run/resume model intent: Option<String> + derived is_some() mask.
MeerkatRunInput.model: Option<String> is re-derived twice (effective model + ResumeOverrideMask.model: bool) (meerkat-mcp-server/src/lib.rs ~2992,3170); absence has 3 meanings (not provided / inherit / clear). Audit says behavior is currently CORRECT (typed mask mitigates). Verify the model field + the two derivation sites + ResumeOverrideMask. Determine whether this should adopt TurnMetadataOverride<T> (ties to H1) or whether the typed mask already discharges it. Propose the minimal typed repair and note the H1 relationship.`,
  },
  {
    id: 'M8',
    label: 'M8:dependencymode-roundtrip',
    text: `M8 (MEDIUM, trivial) — Typed DependencyMode/CollectionPolicyKind laundered through enum-name round-trip.
step_dependency_modes (meerkat-mob/src/runtime/run.rs ~4461) does .as_str() -> match text -> reconstruct the SAME typed enum, with an unreachable "_=>Internal" arm; a direct converter exists ~220 lines above. Test-only callers today. Verify the round-trip, find the existing direct converter, and propose deleting the string round-trip in favor of the direct typed conversion. Confirm no non-test caller depends on the string form.`,
  },
  {
    id: 'M9',
    label: 'M9:flow-timeout-option-string',
    text: `M9 (MEDIUM) — Flow step-timeout terminal-vs-retryable collapsed into Option<String>.
effective_step_timeout returns Option<String> reason; Some => fail flow, None => retry (meerkat-mob/src/runtime/flow.rs ~896). One Option carries lifecycle decision + display text. Verify the function signature + both call sites' branching. Propose a typed enum (e.g. StepTimeoutOutcome{ Retry, Fail{ reason } } or similar) that separates the lifecycle decision from the display string.`,
  },
  {
    id: 'M10',
    label: 'M10:peer-response-terminal-prefix',
    text: `M10 (MEDIUM) — Peer-response-terminal fact reconstructed by source-prefix + text re-parse (OpenAI live).
Producer flattens a typed PeerResponseTerminalFact into source=format!("peer_response_terminal:.."); consumer (meerkat-openai/src/live.rs ~372) does starts_with + split_once("Payload:") + JSON re-parse into realtime instructions. An exact in-repo precedent was already retired ("runtime:steer:"). Verify the producer (find where "peer_response_terminal:" source string is built) and the consumer parse at live.rs. Find the typed PeerResponseTerminalFact owner. Find the retired "runtime:steer:" precedent to mirror. Propose carrying the typed fact through instead of string-encoding it. Flag wire impact (does this cross the comms wire?).`,
  },
  {
    id: 'L1',
    label: 'L1:realmprofilestore-tristate',
    text: `L1 (LOW, trivial) — RealmProfileStore provenance tri-state as Option<store> + explicit: bool.
(meerkat-mob-mcp/src/lib.rs ~221,227) — local builder only, behavior correct. Verify the Option<store> + explicit:bool pair and propose a small provenance enum (e.g. StoreSelection{ Inherited, Explicit(store) }). Confirm it's builder-local (no wire/durable impact).`,
  },
]

const specs = await parallel(
  CLUSTERS.map((c) => () =>
    agent(
      `You are verifying ONE dogma-violation cluster against the CURRENT Meerkat codebase. WORK IN THE CURRENT WORKING DIRECTORY (a git worktree). READ ONLY — make NO edits.

DOGMA (Rule 4, "Truth Is Typed, Identity Is Canonical"): a semantic distinction that matters must be a TYPE. Violation = meaning carried by an untyped representation (string prefix/split, enum-name text, provider-native field, path convention, Value["kind"], error text, Option<T> hiding ownership, clear_*/set_* bool tri-state) AND re-derived into a decision in shared/runtime/surface logic.

CLUSTER ${c.id}:
${c.text}

CRITICAL INSTRUCTIONS:
- The line numbers above are from an audit taken at an OLDER commit; main has since merged the PR, so lines have DRIFTED. Re-trace from current code. Use grep/ripgrep to find the real CURRENT locations.
- Confirm whether the violation STILL EXISTS (set still_present=false if already fixed, and explain in summary).
- Enumerate EVERY current site: producers, consumers, reject-branches, type definitions, wire types, test fixtures. Label each with role.
- Confirm the cited typed owner EXISTS at its current location (or report it's missing and a NEW type must be designed).
- Determine wire_impact (does the changed type serialize on any wire / live in meerkat-contracts / feed SDK codegen? => regen-schemas needed), generated_machine_impact (does it touch meerkat-core/src/generated/*.rs? => the machine SPEC must change and be regenerated, NEVER hand-edit generated files), and durable_metadata_impact (does it change durably persisted SessionMetadata/store shape?).
- Produce ORDERED, CONCRETE repair_steps anchored to real file:line that a coding agent can follow literally. Include the exact new type shape, the serde-boundary handling for any legacy wire form, and which call sites collapse.
- Identify test_targets (crates + test names).

Sweep the WHOLE relevant crate src/, not just the files named above — there may be additional producers/consumers. Return the SPEC_SCHEMA.`,
      { label: c.label, phase: 'Verify', schema: SPEC_SCHEMA }
    )
  )
)

return { infra, specs: specs.filter(Boolean) }
