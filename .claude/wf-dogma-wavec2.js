export const meta = {
  name: 'dogma-357-wavec2-incrate',
  description: 'Finer-lane re-fan-out of the single-crate SNL rows WAVE C under-did; fix in-lane, re-flag only for genuine contracts/machine/cross-crate',
  phases: [{ title: 'InCrate', detail: 'finer file-disjoint lanes, fix in-lane now' }],
};

const PLAN = '/Users/luka/src/meerkat/.claude/worktrees/dogma-2/docs/architecture/dogma-audits/REMEDIATION-PLAN-357.md';
const DESIGNS = '/Users/luka/src/meerkat/.claude/worktrees/ultimate-dogma/.claude/dogma-wavec-snl.json';

const LANES = {
  'mob-actor':     { crates: ['meerkat-mob — files: src/runtime/actor.rs, src/runtime/actor_turn_executor.rs'], rows: [14,181,320] },
  'mob-handle':    { crates: ['meerkat-mob — file: src/runtime/handle.rs'], rows: [282,314] },
  'mob-flow-cond': { crates: ['meerkat-mob — files: src/runtime/conditions.rs, src/runtime/flow.rs, src/definition.rs'], rows: [258,259,260] },
  'mob-supervisor':{ crates: ['meerkat-mob — files: src/runtime/supervisor.rs, src/runtime/supervisor_bridge.rs, src/runtime/topology.rs'], rows: [261,293,334] },
  'mob-misc':      { crates: ['meerkat-mob — files: src/roster.rs, src/profile.rs, src/runtime/bridge_protocol.rs, src/runtime/terminalization.rs'], rows: [70,333,336,126] },
  'rpc-live':      { crates: ['meerkat-rpc — files: src/live_projection_sink.rs, src/handlers/live.rs'], rows: [198,209,355] },
  'rpc-runtime':   { crates: ['meerkat-rpc — file: src/session_runtime.rs'], rows: [97,338,348] },
  'openai':        { crates: ['meerkat-openai'], rows: [51,68,123,149] },
  'provider-misc': { crates: ['meerkat-gemini','meerkat-auth-core','meerkat-live'], rows: [208,8,113] },
  'tools':         { crates: ['meerkat-tools'], rows: [64,202,299] },
  'session':       { crates: ['meerkat-session'], rows: [34,86,225,297] },
  'facade':        { crates: ['meerkat'], rows: [72,167] },
  'core-agent':    { crates: ['meerkat-core — ONLY files under src/agent/'], rows: [166,205,239,272,323,324] },
  'core-data':     { crates: ['meerkat-core — files: src/agent/builder.rs(308), src/types.rs(319)'], rows: [308,319] },
};

const SCHEMA = {
  type: 'object', additionalProperties: false,
  properties: {
    lane: { type: 'string' }, summary: { type: 'string' },
    rows: { type: 'array', items: {
      type: 'object', additionalProperties: false,
      properties: {
        id: { type: 'integer' },
        disposition: { type: 'string', enum: ['FIXED','STILL_NEEDS_LEAD','ALREADY_CLOSED','NOT_APPLICABLE'] },
        evidence: { type: 'string' }, files_changed: { type: 'array', items: { type: 'string' } },
      },
      required: ['id','disposition','evidence'],
    }},
  },
  required: ['lane','rows'],
};

function prompt(lane, cfg) {
  return `You are a Meerkat dogma-remediation IMPLEMENTATION agent on branch worktree-ultimate-dogma.

Dogma doctrine: singular typed authority over stringly/Value/Option-overload; parse-at-boundary into typed enums/newtypes; faults propagated as typed errors not swallowed; surfaces never re-derive truth a seam owns.

YOUR LANE: ${lane}. Edit ONLY: ${cfg.crates.join('; ')}.

These rows were verified LIVE by two prior passes and flagged for follow-up. The earlier pass was CONSERVATIVE and re-flagged in-crate-fixable rows it should have fixed. Your job: FIX them in-lane NOW. Be decisive, not conservative.

Designs: ${DESIGNS} is a JSON array of {id, lane, evidence}. Find your row ids; "evidence" is the precise verified diagnosis + prescribed fix. Full row text: grep "\\*\\*#<id>\\*\\*" "${PLAN}".

YOUR ROWS: ${JSON.stringify(cfg.rows)}

FOR EACH ROW: read the evidence + the current code, confirm the violation, and IMPLEMENT the typed-owner / fail-closed fix within your lane. Add or update a focused gate test (match the crate's test idioms). Then classify:
   - FIXED — implemented in-lane. List files+lines + what you did.
   - STILL_NEEDS_LEAD — ONLY if it genuinely requires (a) a meerkat-contracts wire-type change + regen-schemas, (b) the machine DSL catalog / runtime kernel / make machine-codegen, or (c) a type change in a crate OUTSIDE your lane. Do the safe in-lane part, then describe the precise remaining cross-boundary step. Do NOT edit other crates. Do NOT re-flag merely because a fix is non-trivial — only for a true cross-boundary blocker.
   - ALREADY_CLOSED — the violation is already gone (cite file:line).
   - NOT_APPLICABLE — invalid/obsolete; explain.

ABSOLUTE RULES:
- DO NOT run cargo/make/git/machine-codegen/regen-schemas. Source edits only; the lead builds + fixes cascades + commits.
- Never edit /generated/, specs/ (*.tla), artifacts/schemas/, meerkat-machine-schema/, or any crate outside your lane.
- No back-compat shims/tombstones/aliases (pre-1.0; delete cleanly).
- ABSOLUTELY NO .unwrap()/.expect()/panic!/unreachable!/todo! in non-test code (workspace denies clippy::panic/unwrap_used/expect_used -D warnings). In tests prefer .expect("msg"). Remove imports your edit orphans. NO Option<Option<T>> (use a typed 3-variant enum). NO collapsible nested ifs (use let-chains / match).
- Use ? and typed errors. Match surrounding style. Keep edits minimal + focused.
- Be HONEST and rigorous. Never fabricate. Implement real fixes.

Return one entry per assigned row.`;
}

phase('InCrate');
const names = Object.keys(LANES);
log(`WAVE C2: ${names.length} finer lanes, ${Object.values(LANES).reduce((n,c)=>n+c.rows.length,0)} in-crate rows`);

const results = await parallel(
  names.map((lane) => () =>
    agent(prompt(lane, LANES[lane]), { label: lane, phase: 'InCrate', schema: SCHEMA })
      .then((r) => r || { lane, rows: [], summary: 'AGENT_RETURNED_NULL' })
  )
);

const all = [];
for (const r of results) if (r && Array.isArray(r.rows)) for (const row of r.rows) all.push({ lane: r.lane, ...row });
const byDisp = {};
for (const x of all) byDisp[x.disposition] = (byDisp[x.disposition] || 0) + 1;
log(`WAVE C2 dispositions: ${JSON.stringify(byDisp)}`);

return {
  counts: byDisp, total: all.length,
  fixed_rows: all.filter((x) => x.disposition === 'FIXED').map((x) => ({ id: x.id, lane: x.lane, files: x.files_changed || [], evidence: x.evidence })),
  still_needs_lead: all.filter((x) => x.disposition === 'STILL_NEEDS_LEAD').map((x) => ({ id: x.id, lane: x.lane, evidence: x.evidence })),
  already_closed_ids: all.filter((x) => x.disposition === 'ALREADY_CLOSED').map((x) => x.id).sort((a, b) => a - b),
  not_applicable: all.filter((x) => x.disposition === 'NOT_APPLICABLE').map((x) => ({ id: x.id, lane: x.lane, evidence: x.evidence })),
};
