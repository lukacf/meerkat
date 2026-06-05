export const meta = {
  name: 'dogma-review',
  description: 'Adversarial dogma review of the folklore-cleanup diff: verify each fix genuinely deletes the re-derivation, adopts a canonical typed owner, introduces no new folklore/shim/fail-open, and preserves behavior',
  phases: [{ title: 'Review', detail: 'one adversarial reviewer per wave-commit diff' }, { title: 'Synthesize', detail: 'cluster confirmed findings' }],
}

const DOGMA = `Meerkat Dogma Rule 4 (Truth Is Typed, Identity Is Canonical): a semantic distinction that drives a DECISION must be carried by a TYPE, not a string-prefix/split, enum-name text matched via as_str()/==, Value["kind"], error-message substring, path/name convention, Option<T> hiding ownership, or clear_*/set_* bool tri-state. Also Rule 8: no silent success/fail-open; soft-vs-hard and terminal classes must be explicit. A GENUINE fix: introduces/adopts the canonical typed owner AND DELETES the string/Value/Option re-derivation at the decision point. A FAKE/incomplete fix: adds a type but leaves the old string match still driving a decision; or keeps a fail-open \`_ =>\` arm; or leaves a back-compat shim that still feeds decisions (vs a legitimate parse-at-boundary); or silently changes behavior; or introduces NEW folklore (e.g. a new string sentinel, a new Option-hiding-ownership). Legitimate (NOT findings): parse-at-a-boundary into a type, display/log strings, provider-native fields used strictly inside a provider crate, a UUID kept as a string on the wire with a typed in-memory view, Option whose absence has exactly one harmless meaning.`

const SCHEMA = {
  type: 'object', additionalProperties: false,
  required: ['wave', 'verdict', 'confirmed_findings', 'notes'],
  properties: {
    wave: { type: 'string' },
    verdict: { type: 'string', description: 'clean | findings' },
    confirmed_findings: {
      type: 'array',
      items: {
        type: 'object', additionalProperties: false,
        required: ['file', 'line', 'severity', 'issue', 'evidence'],
        properties: {
          file: { type: 'string' },
          line: { type: 'integer' },
          severity: { type: 'string', description: 'high | medium | low' },
          issue: { type: 'string', description: 'what dogma failure: fake-fix / residual-re-derivation / new-fail-open / shim-drives-decision / new-folklore / behavior-change' },
          evidence: { type: 'string', description: 'concrete code citation proving it (the reviewer must have READ the current code, not the diff alone)' },
        },
      },
    },
    notes: { type: 'string' },
  },
}

phase('Review')

const WAVES = [
  { commit: '811e55ff0', label: 'review:H1', scope: 'H1 — LLM-identity tri-state -> TurnMetadataOverride; machine DSL collapse (session_document) + both generated files + wire MobTurnStartParams. Verify: the (Option<T>,clear_*:bool) pairs are GONE (no clear_provider_params/clear_auth_binding struct fields driving decisions), set+clear is structurally unrepresentable, legacy clear_* is folded ONCE at serde boundaries (legitimate) not re-derived in logic, the DSL ClearAndSet reject transitions/variants are deleted (not just unused), and the generated files match the spec (no hand-edit).' },
  { commit: '3dd9ba0b8', label: 'review:wave1', scope: 'mob (M3/M8/M9/N5/N10) + mcp (M7/S4/N9) + workgraph. Verify: FLOW_SYSTEM prefix .starts_with replaced by typed AgentIdentity predicate (no residual prefix-match driving admission); flow_run enum-name round-trips replaced by direct typed converters (dead Internal arms gone); StepTimeoutOutcome separates lifecycle from display (no Option<String> conflation left); WiringSides/FlowRef replace &[&str]/string-split (no residual .contains("local") decision); StreamReadTimeout tagged enum (no no_timeout+timeout_ms precedence reconstruction); typed StatusCode (no 401/403 substring); WorkEvidenceKind (no kind.as_str re-match / trust-bypass).' },
  { commit: '1360a3287', label: 'review:wave2a', scope: 'connection-resolver (N3/N4/S3) + workgraph-tool-error (N7) + store-selection (L1). Verify: BindingOrigin typed (is_env_default matches the variant, NOT realm=="env_default" slug equality; no residual magic-slug decision); provider_default typed marker (no residual format!("default_{provider}") + string-match selection); RealmConnectionSet.realm_id is RealmId (no as_str compare); WorkGraphToolErrorCode (no code.as_str()->jsonrpc with silent -32603); RealmProfileStoreSelection enum (no Option+explicit:bool). Also verify the ~30-site origin/provider_default ripple did not silently set the WRONG variant anywhere (Configured vs SyntheticEnvDefault).' },
  { commit: '77c3a89b8', label: 'review:wave2b', scope: 'provider-identity (N2/S6) + contracts DTOs (N6/N7/N8) + openai-live (M10/N5). Verify: LlmClient::provider() returns Provider (no &str-then-from_name round-trip; adapter match exhaustive, NO fail-open _ => tag); WireMobWireAction/WireMobLifecycleStatus/etc typed (no action/status:String re-match); RevisionSelector/WireInitialTurn (no "current"/"deferred" Value sentinel); M10 PeerResponseTerminalFact carried TYPED end-to-end and the live.rs starts_with/split_once("Payload:") parse + source=format!("peer_response_terminal:..") flatten are DELETED (confirm by reading live.rs + input.rs); N5 provider-swap guard is Provider==Provider not String!=.' },
  { commit: 'e3d11d6fc', label: 'review:wave2c-auth', scope: 'auth (M5/N1/N9/S5/S7). Verify: backend-kind gate compares NormalizedBackendKind (no String !=); NormalizedAuthMethod::persisted_auth_mode is the owner and resolver/create-paths use binding.auth() (the persisted_auth_mode_for_auth_method(&str) shim is parse-at-boundary ONLY, not a decision oracle); mcp_oauth refresh uses RefreshFailureObservation::requires_reauth (no body.contains("invalid_grant")); REST provider compare typed; CLI LoginProvider typed + is_legacy_chatgpt_base_url single owner. CONFIRM the N1 create-path bug-fix did not over-open (oauth-login methods still rejected at create).' },
  { commit: '85bd6ab0f', label: 'review:wave2d-comms', scope: 'comms-notice (M4/S2). Verify: SystemNoticeBlock::Comms.kind is CommsNoticeKind (no kind.as_str() decision; the fail-open _ => in model_projection_text is DELETED and the match is exhaustive; if Other(String) exists it is matched explicitly not silently); SystemNoticePeer.id is PeerId (the PeerId::parse Ok/Err projection branch is deleted; producers parse once). Confirm wire tag values byte-match the legacy strings (no silent wire break) and projection text is unchanged.' },
  { commit: '1a8c21e17', label: 'review:wave2e1', scope: 'H2 (mob identity) + S1 (reserved-key). Verify: persisted_mob_binding reads metadata.mob_member_binding DIRECTLY (no comms_name split, no "mob:" colon, no magic-key peer_meta cross-check); MemberCommsName FromStr is fail-closed (3 components, validated); ONE shared mob_realm_id helper used by producer+consumer (dot/colon divergence gone); PeerRole::External replaces the "external" string; mob_member_binding is serde-default (back-read safe); comms_name retained ONLY as transport name (not identity). Confirm the dot/colon bug fix makes owns_persisted_bridge_session fire via the typed binding. S1: ReservedMetadataKey::classify is the single authority (no scattered starts_with("meerkat.")).' },
  { commit: '8da4c990c', label: 'review:wave2e2', scope: 'M1/M2/M6. Verify: SessionLifecycleTerminal typed reserved-key (no Value::Bool terminality re-derivation in the standalone path; metadata_marks_archived deleted; legacy session_archived bool back-read; runtime path still uses RuntimeState::Retired); AgentError::DurableSnapshotSyncUnsupported typed (no message.contains substring); SessionMetadata.realm_id is Option<RealmId> parsed once at boundary (no parallel Option<String>; RealmId serializes transparently = no wire break). Confirm M6 left ConfigEnvelope.realm_id (on emitted contracts) as String intentionally (no half-typed contract). Flag any realm_id consumer that still re-parses a String late.' },
]

const reviews = await parallel(
  WAVES.map((w) => () =>
    agent(
      `You are an adversarial Meerkat DOGMA REVIEWER. WORK IN THE CURRENT WORKING DIRECTORY (main git worktree). READ ONLY.

${DOGMA}

YOUR TARGET: commit ${w.commit} (one wave of the folklore cleanup).
SCOPE / what to verify: ${w.scope}

METHOD (be adversarial + evidence-based — this is the final gate for an RMAT-fundamentalist reviewer who distrusts "done" claims):
1. \`git show ${w.commit} --stat\` then read the actual diff (\`git show ${w.commit}\`) to see what changed.
2. For EACH claimed fix, READ THE CURRENT CODE (not just the diff) at the decision point. Confirm the string/Value/Option re-derivation is ACTUALLY DELETED at the point where a decision is made — not merely that a type was added alongside it.
3. Hunt specifically for: (a) a typed owner added but the OLD string match still drives a real branch somewhere; (b) a fail-open \`_ =>\` / \`unwrap_or(default)\` that silently swallows an unknown variant into a decision; (c) a "shim" that still feeds a decision rather than only parsing at a boundary; (d) a NEW folklore introduced by the fix (new string sentinel, new Option-hiding-ownership, new magic key); (e) a silent behavior change vs the documented intent.
4. Only report a finding you can back with a CONCRETE current-code citation (file:line + the offending expression). Do NOT report legitimate carve-outs (parse-at-boundary, display strings, provider-native-inside-provider-crate, single-meaning Option, UUID-string-on-wire).

Return SCHEMA. verdict="clean" only if you found NO genuine dogma failure after reading the real code.`,
      { label: w.label, phase: 'Review', schema: SCHEMA }
    )
  )
)

phase('Synthesize')
const allFindings = reviews.filter(Boolean).flatMap((r) => (r.confirmed_findings || []).map((f) => ({ ...f, wave: r.wave })))
const synthesis = await agent(
  `You are the synthesis reviewer. Below are confirmed dogma findings from per-wave adversarial review of the folklore cleanup (commits 811e55ff0..8da4c990c). Cluster them by severity + root shape, dedup, and give a final verdict: is the cleanup genuinely dogma-clean, or are there real residual violations the lead must fix? For each real finding give file:line + the exact fix. Distinguish genuine violations from false-positives (re-verify the load-bearing ones against the cited code yourself).

FINDINGS (JSON):
${JSON.stringify(allFindings, null, 1)}

Return a thorough markdown report as your final text.`,
  { label: 'synthesize', phase: 'Synthesize' }
)

return { perWave: reviews.filter(Boolean).map((r) => ({ wave: r.wave, verdict: r.verdict, count: (r.confirmed_findings || []).length })), totalFindings: allFindings.length, findings: allFindings, synthesis }
