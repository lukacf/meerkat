export const meta = {
  name: 'folklore-wave2a',
  description: 'Parallel write-only: self-contained hub subsystems (connection-resolver, workgraph-tool-error, RealmProfileStoreSelection)',
  phases: [{ title: 'Implement', detail: 'one agent per disjoint subsystem, write-only' }],
}

const COMMON = `You are fixing Rule-4 String/JSON/Option folklore in the Meerkat repo. WORK IN THE CURRENT WORKING DIRECTORY (main git worktree; H1 + wave1 already committed). No prior context is inherited.

DOGMA Rule 4: a semantic distinction that drives a DECISION must be a TYPE, not a string/enum-name-text/Value-kind/path-convention/Option-hiding-ownership/clear-bool. Fix = introduce/adopt the typed owner and DELETE the re-derivation. Legitimate (don't touch): parse-at-boundary-into-type, display/log strings, provider-native inside provider crates, Option whose absence has exactly one harmless meaning.

READ for exact per-site detail (current file:line, typed owner, ordered repair):
- .claude/folklore-specs/resweep_synthesis.md   (clusters N1-N10 + singletons S1-S7, ordered repairs)
- .claude/folklore-specs/resweep_new.json        (per-site file/line/typed_owner/repair_sketch/impact)
- .claude/folklore-specs/clusters.json           (13 confirmed clusters incl. L1)

RULES:
- WRITE-ONLY: edit source, but do NOT run cargo/build/test/make (the lead builds + corrects). Read/grep freely.
- Stay STRICTLY within your assigned files/crates. Do NOT edit anything outside them, and nothing under */generated/ or specs/.
- No .unwrap()/.expect()/panic! in non-test library code.
- Preserve behavior; only representation changes. Update your crate's own tests.
- If a fix needs a file outside your lane, STOP that site and report it in deferred_cross_crate.
Return the structured report.`

const SCHEMA = {
  type: 'object', additionalProperties: false,
  required: ['lane', 'sites_fixed', 'files_edited', 'new_types', 'deferred_cross_crate', 'notes'],
  properties: {
    lane: { type: 'string' },
    sites_fixed: { type: 'array', items: { type: 'string' } },
    files_edited: { type: 'array', items: { type: 'string' } },
    new_types: { type: 'array', items: { type: 'string' } },
    deferred_cross_crate: { type: 'array', items: { type: 'string' } },
    notes: { type: 'string' },
  },
}

phase('Implement')

const LANES = [
  {
    label: 'wave2a:connection',
    text: `LANE = the connection/realm binding resolver. Files you may edit: meerkat-core/src/connection.rs, meerkat-llm-core/src/provider_runtime/registry.rs, meerkat-web-runtime/src/lib.rs. (Do NOT touch meerkat/src/factory.rs — keep is_env_default()'s bool signature so factory callers stay valid.)
Fix (read resweep_synthesis.md clusters N3, N4 and singleton S3):
- N3 (connection.rs ~137/446/851): "synthetic env-var default binding" recovered by string equality on RealmId/BindingId slugs ("env_default"/"default"). Introduce a typed origin discriminant: enum BindingOrigin { Configured, SyntheticEnvDefault } carried on AuthBindingRef (serde default = Configured for back-read; wire-additive). Stamp SyntheticEnvDefault in synthesize_default_from_spec; stamp Configured at the configured-materialize seam. Rewrite is_env_default() to matches!(self.origin, SyntheticEnvDefault) — KEEP its bool signature so callers (incl. out-of-lane factory.rs) are unchanged. connection.rs:446 short-circuit matches the variant. The synthetic case no longer needs to mint the magic "env_default" slug.
- N4 (connection.rs ~516/984/1021): "this binding is the default for provider X" carried only by the default_<provider> NAME. Add a typed per-binding default marker (e.g. provider_default: bool on the provider-binding config struct, or a DefaultRole). from_inline_api_keys sets it per provider (not just idx==0). selected_binding_id_for_provider scans for the typed marker instead of format!("default_{provider}") + string-match. meerkat-web-runtime/src/lib.rs:~314 re-synthesizes format!("default_{provider}") to look up a backend — change it to look up by the typed marker / explicit id. BindingId becomes pure opaque identity.
- S3 (registry.rs:208): auth_binding.realm.as_str() != realm.realm_id — typed RealmId downgraded to &str vs the bare String field RealmConnectionSet.realm_id. Retype RealmConnectionSet.realm_id to RealmId (parse at from_config); line 208 becomes RealmId != RealmId; remove the redundant re-parse at connection.rs:~644.
NOTE: AuthBindingRef is Serialize/Deserialize and lease identity is persisted — keep the new origin field serde(default) so old rows read as Configured. Update connection.rs/registry tests. Report (don't fix) any factory.rs change you think is needed.`,
  },
  {
    label: 'wave2a:workgraph-tool-error',
    text: `LANE = the WorkGraph tool-error code (cluster N7, the workgraph-tool-error member only). Files you may edit: meerkat-workgraph/src/** and meerkat-mcp-server/src/lib.rs (the map_workgraph_tool_error region + WorkGraphToolError definition).
Fix (resweep_synthesis.md N7 first member; resweep_new.json mcp-server/src/lib.rs:2066):
- WorkGraphToolError.code: String is collapsed from the 9-variant WorkGraphError; the surface map_workgraph_tool_error re-matches error.code.as_str() to WORKGRAPH_TOOL_* constants -> JSON-RPC numeric code with a silent _ => -32603.
- Introduce a typed WorkGraphToolErrorCode (derive it from WorkGraphError — define the enum where WorkGraphError lives if that's meerkat-workgraph, else in mcp-server). Build the typed code directly from WorkGraphError in map_error(); add a jsonrpc_code() method (reuse meerkat_contracts::ErrorCode::jsonrpc_code() where aligned, as two arms already do). Replace WorkGraphToolError.code: String with the typed code; delete the surface error.code.as_str() match and the silent -32603 fall-through (make it exhaustive).
Update affected tests in workgraph + mcp-server. If this requires a NEW type in meerkat-contracts, define it there ONLY IF you can also wire it without editing other crates; otherwise report deferred.`,
  },
  {
    label: 'wave2a:l1-store-selection',
    text: `LANE = meerkat-mob-mcp/src/lib.rs ONLY, cluster L1 (clusters.json). Replace the realm-profile-store provenance tri-state — realm_profile_store: Option<Arc<dyn RealmProfileStore>> + parallel realm_profile_store_explicit: bool (re-derived at the auto-upgrade site ~:283) — with a single typed enum field, e.g. RealmProfileStoreSelection { Default(Arc<dyn RealmProfileStore>), Explicit(Arc<dyn RealmProfileStore>) } (or Inherited/Default/Explicit as fits), so "why the store has its value" is typed, not a parallel bool. The auto-upgrade-to-SQLite decision at ~:283 matches the variant instead of !explicit. Builder-local, no wire/durable impact. Update mob-mcp's own tests. Do NOT touch H2-related comms_name/persisted_mob_binding code (a later wave owns that).`,
  },
]

const reports = await parallel(
  LANES.map((l) => () => agent(`${COMMON}\n\n${l.text}`, { label: l.label, phase: 'Implement', schema: SCHEMA }))
)
return { reports: reports.filter(Boolean) }
