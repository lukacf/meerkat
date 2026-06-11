export const meta = {
  name: 'dogma-mechanical-m1',
  description: 'Dogma-357 mechanical fan-out wave M1: 10 disjoint single-crate lanes, write-only',
  phases: [{ title: 'Fix' }],
}

// Write-only fan-out. Each agent edits ONE crate lane, NO cargo, NO git.
// The lead (me) builds, corrects cascades, runs gates, and commits afterward.

const RULES = `
You are a WRITE-ONLY implementation agent inside a Rust workspace (Meerkat / rkat).
Project doctrine (dogma) — apply rigorously:
- Singular TYPED authority: replace stringly/Value/JSON/tuple state with typed owners (enums, newtypes) that parse-at-boundary and FAIL CLOSED on unknown. Never fail-open.
- NO back-compat shims (this is pre-1.0): delete cleanly; no migration tombstones, no "legacy" duplicate fields, no deprecation aliases.
- NO panic!/unwrap()/expect() in library (non-test) code. Use Result/? propagation or explicit match. In #[cfg(test)] code, unwrap/expect is fine.
- Errors propagate as typed variants; do not swallow with .ok()/unwrap_or default that hides a fault.
- Match the surrounding code's idiom, naming, comment density. Add a focused #[test] (the "Gate" in each row) proving the fix.
- Newtypes get fail-closed constructors/FromStr/Deserialize; reject malformed input at the boundary, not later.

HARD CONSTRAINTS:
- Do NOT run cargo, make, git, or any build/test command. You cannot build. Write code only.
- Edit ONLY files inside your assigned crate lane (listed below). Do not touch other crates, meerkat-core, meerkat-contracts, or the facade unless explicitly told.
- Do NOT add new dependencies to Cargo.toml.
- If a row is ALREADY satisfied in the current code (verify by reading), do NOT change it — report disposition ALREADY_CLOSED with the file:line evidence proving it.
- If a row's fix genuinely requires editing a crate OUTSIDE your lane, do the in-lane part and report the cross-crate need in 'blockers' — do NOT edit out-of-lane files.

Return (StructuredOutput) a per-row disposition with concrete file paths and a 1-2 line evidence note each.
`

const SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['lane', 'rows'],
  properties: {
    lane: { type: 'string' },
    rows: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['row', 'disposition', 'files', 'summary'],
        properties: {
          row: { type: 'string', description: 'e.g. "#37"' },
          disposition: { type: 'string', enum: ['FIXED', 'ALREADY_CLOSED', 'BLOCKED'] },
          files: { type: 'array', items: { type: 'string' } },
          summary: { type: 'string' },
          evidence: { type: 'string', description: 'file:line proof for ALREADY_CLOSED, or blocker detail' },
          test_added: { type: 'string', description: 'name of the gate test added, if any' },
        },
      },
    },
    blockers: { type: 'array', items: { type: 'string' } },
  },
}

const LANES = [
  {
    label: 'mob-pack',
    crate: 'meerkat-mob-pack (src/signing.rs, src/trust.rs, src/manifest.rs, src/archive.rs, src/exec_bits.rs + a typed-vocabulary module)',
    spec: `
#37 Mobpack signer trust identity is string-keyed. In signing.rs:~5 (PackSignature) and trust.rs:~24 (TrustedSigners.signers: BTreeMap<String,String>), introduce SignerId, Ed25519PublicKeyHex, Ed25519SignatureHex, Rfc3339Timestamp newtypes that parse-validate at TOML deserialization; key TrustedSigners by SignerId and store typed values so lookup/verify never re-parses raw hex inline. Gate: signing/trust round-trip tests over typed PackSignature/TrustedSigners; a fail-closed test that malformed public_key/signature hex is rejected at deserialization (not at verify time).

#74 Mobpack manifest selectors are string authorities. In manifest.rs:~12-46, replace models: BTreeMap<String,String>, surfaces: BTreeSet<String>, ProfileSection.model: Option<String>/skills: Vec<String> with typed newtypes (ModelAlias, ModelRef, SurfaceSelector, SkillPath) deserialized-once at the TOML boundary. Gate: manifest round-trip test over typed selectors; fail-closed test that an unknown surface selector is rejected at deserialize.

#95 Mobpack archive section/exec semantics use path folklore. In archive.rs:~55 replace path.starts_with("skills/"|"hooks/"|"mcp/"|"config/") with a typed ArchiveSection::classify(path) -> Option<ArchiveSection> owning the section vocabulary; in exec_bits.rs replace shebang/suffix/path executable inference with a typed ExecPolicy classifier; route digest identity through a typed CanonicalPath newtype. Gate: archive test with section classification table-driven from ArchiveSection (no string-prefix literal in production); exec-policy test over the typed classifier.

NOTE: #74 mentions ModelRef/ModelAlias — define them LOCALLY in meerkat-mob-pack (a typed-vocabulary module), do NOT import from other crates. Keep all new types in meerkat-mob-pack.`,
  },
  {
    label: 'skills',
    crate: 'meerkat-skills (src/source/remote.rs, src/source/git.rs, src/source/embedded.rs, src/renderer.rs) — do NOT edit meerkat-core',
    spec: `
#36 Invalid skill quarantine fabricates canonical-looking SkillKey. remote.rs:~163 (fallback_key/invalid name) and git.rs:~200 (invalid_skill_key) build a synthetic SkillKey for quarantine diagnostics that looks loadable. Replace with a typed QuarantinedSkill { source_uuid, raw_id, reason } that is NOT a loadable SkillKey; ensure load() can never resolve a quarantine placeholder key. Gate: test that load(invalid/quarantine key) returns NotFound; quarantine diagnostics carry raw_id+reason in a non-loadable type.

#42 Skill prompt rendering reintroduces slash SkillKey folklore. renderer.rs:~84,130 render skill identity as {source_uuid}/{skill_name} string ids in XML (SkillKey Display). Emit explicit typed attributes (source_uuid + skill_name as separate XML attrs, matching the typed load_skill tool contract) instead of a single re-parseable slash id. Gate: test that rendered skill block exposes source_uuid and skill_name as distinct attributes mapping 1:1 to load_skill's SkillKey args (no slash re-parse).

#77 Embedded skill registrations derive builtin identity from path suffix. embedded.rs:~13-20,97 derive SkillKey from reg.id.rsplit('/').next() (final slash segment), so distinct static ids with different prefixes collapse. Carry a typed RegistrationId (fail-closed parse) and either reject collisions or namespace by full registration id; load() (~97) must match the full typed id, not the trailing segment. Gate: test that two embedded registrations 'a/foo' and 'b/foo' do NOT collapse to one loadable key (distinct or rejected at collect time).`,
  },
  {
    label: 'tools',
    crate: 'meerkat-tools (src/builtin/composite.rs)',
    spec: `
#318 Skill builtins bypass their own disabled-by-default policy. In register_skill_tools (composite.rs:~285-290), gate each skill tool through resolved_policy.is_enabled(name, tool.default_enabled()) like the other builtin paths (composite.rs:~198/:257/:356) and the sibling register_image_generation_tool (which honors visibility.resolve at :300), instead of unconditionally inserting every skill tool name into allowed_tools. Skill tools (browse.rs default_enabled()->false) then stay off unless explicitly enabled. Gate: test that with default policy, browse_skills/load_skill are absent from the dispatcher catalog (respecting default_enabled()==false); with explicit policy enable they appear.`,
  },
  {
    label: 'store',
    crate: 'meerkat-store (src/realm.rs)',
    spec: `
#335 Realm store paths can alias requested realm identity before typed validation. Parse RealmId::parse(realm_id) (realm.rs:~575 — currently only on the create path) at the TOP of ensure_realm_manifest_in, before deriving the filesystem path. Then on the existing-manifest branches (realm.rs:~501-508 and :554-561) verify existing.realm == parsed_realm and return a typed StoreError::RealmIdentityMismatch on inequality (two raw strings sanitizing to the same path — e.g. a.b and a_b — must not silently share one manifest). Fail closed before backend-hint validation. Gate: test that open_or_create with raw a.b, then open with a_b (same sanitized path) returns a typed identity-mismatch error, not the first manifest. Existing realm manifest tests stay green.`,
  },
  {
    label: 'rpc-clean',
    crate: 'meerkat-rpc (src/session_runtime.rs, src/handlers/live.rs) — do NOT edit meerkat-contracts',
    spec: `
#61/#67 Direct session surfaces own model default policy / re-infer provider before factory. In session_runtime.rs:~5711 DELETE the .or_else(|| Provider::infer_from_model(&build_config.model)); resolve provider through registered model ownership exactly as REST does at meerkat-rest/src/lib.rs:~1038 (entry.provider + provider_override_mismatch_reason), failing closed when neither explicit provider nor registered owner is present. This is the only RPC infer_from_model callsite — grep meerkat-rpc/src to confirm zero remain after. Gate: RPC session/create with model string + no provider + no registered owner returns an error (mirror REST); existing runtime_turn_metadata_model_override_uses_resolved_provider_hint test still passes.

#302 Router live open can succeed without provider adapter. At handlers/live.rs:~929-970 the 'if let Some(factory)' block (precheck + adapter attach) is skipped when session_factory is None, yet open_channel_with_authority already opened a channel and returns a bootstrap + channel id with all-false capabilities. When no factory is wired, fail the open with a typed error and abandon the admission (open no channel) instead of returning a channel id that later rejects real commands. Gate: test that live/open without a session factory returns a typed error and registers/opens no channel.`,
  },
  {
    label: 'xtask-scripts',
    crate: 'scripts/ and Makefile and meerkat-machine-codegen/BUILD.bazel — shell/make only, NO Rust crate edits',
    spec: `
#285 Schema freshness gate mutates artifacts while validating. scripts/verify-schema-freshness.sh creates TEMP_ROOT/FRESH_DIR (lines ~16-31) but emit-schemas writes in-place to artifacts/schemas/, then diffs git-HEAD vs the now-mutated tree (~line 46). Run emit-schemas with cwd=TEMP_ROOT (or copy artifacts/schemas to TEMP_ROOT first and emit there) and diff committed-vs-fresh in the temp dir, leaving the workspace clean. Gate: after running verify-schema-freshness.sh, git status shows no modified artifacts/schemas files; stale schemas still detected.

#227 Bazel package-version metadata is outside version parity. meerkat-machine-codegen/BUILD.bazel:~44 hardcodes CARGO_PKG_VERSION '0.7.0-alpha.0'. Generate it (and the Web runtime EXPECTED_VERSION) from workspace.package.version during make regen-schemas, and add an assertion for both to scripts/verify-version-parity.sh. Gate: bump workspace version without regenerating and confirm verify-version-parity.sh fails on the Bazel CARGO_PKG_VERSION and WASM EXPECTED_VERSION mismatch (currently passes). (Note: verify-version-parity.sh may already check WASM EXPECTED_VERSION — if so, only add the Bazel BUILD.bazel CARGO_PKG_VERSION assertion and leave the WASM check intact.)

#203 BuildBuddy change detection skips governance-audit source changes. Add governance source to the machine_authority_path allowlist in scripts/machine-authority-changed (~lines 83-100): xtask/src/rmat_policy.rs, xtask/src/seam_inventory.rs, xtask/src/ownership_ledger.rs, xtask/src/rmat_audit.rs; and to the mark_all-triggering cases in scripts/buildbuddy-edge-changes (~lines 32-40): governance baselines + dogma docs. Gate: a diff touching xtask/src/rmat_policy.rs makes scripts/machine-authority-changed exit 0 (changed) instead of 'machine-authority unchanged'.

#296 Agent gate ignores formal/governance artifacts. is_rust_relevant_path (scripts/cargo-agent-gate:~135) and is_relevant_path (scripts/buildbuddy-agent-gate:~158) only match *.rs|Cargo.*|Makefile|scripts. Add a governance/spec allowlist (specs/**, governance baselines, .claude/skills/meerkat-dogma-inquisition/**, dogma docs) so those path changes route formal/governance checks instead of exiting 0. Gate: a diff touching specs/machines/** or a dogma doc makes make agent-gate run the formal/governance checks instead of 'no build-relevant changes detected'.

#266 Dogma skill doctrine mirror is ungated in make ci. scripts/sync-meerkat-dogma-skill-docs.sh already IS a cmp-based drift gate (exits 1 on divergence) but is absent from the make ci target (Makefile:~328). Add 'sync-meerkat-dogma-skill-docs' (or a verify wrapper) to the ci AND ci-smoke target lists. Gate: edit docs/architecture/meerkat-dogma.md without re-syncing and confirm make ci fails via the dogma sync gate.

#221 BuildBuddy machine-authority lane can pass without TLC. In xtask/tests/machine_verify_all_tlc_test.sh:~22, do not silently exec 'machine-check-drift --all' and exit 0 when tlc is absent on a lane that advertises TLC-backed verification; fail closed (require tlc on the TLC lane) OR rename the lane to drift-only and update scripts/buildbuddy-doctor:~574 to stop blessing the no-TLC shape as 'machine-verify/TLC'. Gate: run the lane with tlc removed from PATH and assert it fails (or is clearly labeled drift-only).`,
  },
  {
    label: 'sdks-consume',
    crate: 'sdks/typescript/src/client.ts, sdks/web/src/mob.ts — TypeScript only, consume already-generated types',
    spec: `
#15 (SDK last-mile only) RPC catalog leaves skills schema-light in TS. In sdks/typescript/src/client.ts:~1070, listSkills(): Array<Record<string,unknown>> should consume the generated SkillListResponse/WireSkill type from sdks/typescript/src/generated/ instead of returning untyped records; throw on malformed. Gate: TS test asserts listSkills returns typed records and throws on malformed. (Do NOT edit Rust or generate.py — only consume the existing generated TS type. If the generated type is absent, report BLOCKED.)

#89 (web SDK side only) Resolved model capabilities are SDK-defaulted. In sdks/web/src/mob.ts:~238 parseResolvedModelCapabilities, replace Boolean(record.x) coercion with strict consumption of the generated WireResolvedModelCapabilities (sdks/web/src/generated/mob.ts), throwing on malformed/absent booleans (mirror the existing parseWireHandlingMode pattern in the same file). Gate: web SDK test that a malformed boolean throws, does not coerce. (Do NOT edit Rust — only the web SDK TS consumer.)`,
  },
  {
    label: 'mob-mcp',
    crate: 'meerkat-mob-mcp (src/agent_tools.rs) — define schemars input structs locally in this crate',
    spec: `
#32 Agent-facing mob MCP schemas are handwritten JSON contracts. In agent_tools.rs:~1423-1435 (tool_def + tool_config_schema/inline_profile_schema/spawn_tooling_schema/mob_definition_schema) replace the hand-authored json!({...}) schema bodies with schemars-derived input structs generated via a typed_schema::<T>()-style helper (see how meerkat-mob-mcp/src/public_mcp.rs or meerkat-tools::schema_for uses schemars). Define the input structs in THIS crate (meerkat-mob-mcp) as #[derive(JsonSchema, Deserialize)] structs that mirror the existing JSON shape exactly (same field names/required-ness), then emit schema_for::<T>() instead of the json! literal. Delete the json! schema literals. Gate: a #[test] asserting each agent tool's input_schema equals schema_for::<T>() (no json! literal); the schemas must remain shape-compatible with the prior hand-authored ones.`,
  },
  {
    label: 'memory',
    crate: 'meerkat-memory (src/tool.rs, src/hnsw.rs) and docs/guides/memory.mdx',
    spec: `
#50 Memory search public contract loses owner/source handle (DRIFT — doc fix). Memory search is session-SCOPED in code (store.search(&self.scope,..); results carry metadata.session_id but never cross sessions). The runtime emits only {content,score,turn} (tool.rs:~130-134). Fix the DRIFT by deleting the phantom top-level session_id field claim and the 'enables cross-session recall' claim from docs/guides/memory.mdx:~176,190-192 to match delivery. Do NOT change the scope to cross-session. Gate: memory.mdx documented fields == keys emitted at tool.rs:~130; no session_id/cross-session claim remains.

#240 HNSW memory rollback leaves stale search candidates (live-window hardening). In index_scoped_batch (hnsw.rs:~240-349): when the in-memory HNSW insert fails AFTER tx.commit() and the DB rows are deleted (~312-331), keep the live scoped index consistent with the DB (track the batch's point_ids and rebuild/repair the scoped ScopedHnswIndex, OR stage the HNSW insert before committing SQLite) so search (~351-429) never selects neighbor slots for DB-deleted points that then hit None=>continue (~395/:409). Crash recovery is already symmetric (next_id re-derived; index rebuilt on open) so this is a live-only hardening. Gate: test that forces a partial HNSW failure mid-batch; a subsequent search in the same live process returns no phantom (DB-deleted) neighbor slots.`,
  },
  {
    label: 'rest',
    crate: 'meerkat-rest (src/lib.rs, src/auth_endpoints.rs) — do NOT edit meerkat-cli',
    spec: `
#342 REST host-info advertises feature-disabled REST paths. At lib.rs:~3278 options.rest_paths is populated from the full static rest_path_catalog() (lists mob/mcp/schedule routes unconditionally) while the axum router registers them behind cargo feature gates and options.mobs/mcp_live/schedules are already cfg!-gated. Make rest_paths derive from the SAME cfg-gated option set (filter the catalog by enabled features). surface.rs:~261 build_runtime_host_info just relays options.rest_paths. Gate: test that host_info on a no-mob/no-mcp/no-schedule build omits those rest_paths.

#317 (REST part only) Auth test surface mutates lease truth. REST test_auth_binding (auth_endpoints.rs:~947/:977) calls publish_resolved_auth_lease, so a validation surface advances AuthMachine lease freshness; route the test through a dry-run/non-publishing resolution that returns validation status WITHOUT publishing the lease. (Only the REST side here; the CLI 'rkat auth test' part is out of this lane — note it as a blocker if it shares a helper.) Gate: test that test_auth_binding does not change the AuthMachine lease epoch/freshness (read-only resolution).`,
  },
]

phase('Fix')
const results = await parallel(
  LANES.map((lane) => () =>
    agent(
      `${RULES}\n\n=== YOUR LANE: ${lane.label} ===\nAssigned crate/files: ${lane.crate}\n\nRows to close (verbatim spec; verify-first against current code, implement-if-live, else ALREADY_CLOSED with evidence):\n${lane.spec}`,
      { label: `m1:${lane.label}`, phase: 'Fix', schema: SCHEMA }
    )
  )
)

return results.filter(Boolean)
