export const meta = {
  name: 'dogma-357-wavec-cascade',
  description: 'Implement the 106 verified-live typed-owner cascade fixes, routed to true-owner crates, designs from the prior verify pass',
  phases: [{ title: 'Cascade', detail: 'one agent per true-owner crate-lane: implement the verified fix' }],
};

const PLAN = '/Users/luka/src/meerkat/.claude/worktrees/dogma-2/docs/architecture/dogma-audits/REMEDIATION-PLAN-357.md';
const DESIGNS = '/Users/luka/src/meerkat/.claude/worktrees/ultimate-dogma/.claude/dogma-needs-lead.json';

const LANES = {
  'C-core-agent': { crates: ['meerkat-core — ONLY files under src/agent/'], rows: [25,120,128,129,166,205,239,272,273,274,276,309,323,324] },
  'C-core-data':  { crates: ['meerkat-core — all files EXCEPT src/agent/'], rows: [1,76,93,103,132,141,142,143,178,207,233,253,254,270,275,308,312,319,322,331,332,356] },
  'C-mob':        { crates: ['meerkat-mob'], rows: [14,70,126,181,258,259,260,261,282,293,314,320,333,334,336] },
  'C-rpc':        { crates: ['meerkat-rpc'], rows: [97,115,198,199,209,249,301,326,338,346,348,355] },
  'C-runtime':    { crates: ['meerkat-runtime'], rows: [2,24,105,106,263,267,288] },
  'C-realtime':   { crates: ['meerkat-openai','meerkat-gemini','meerkat-live'], rows: [51,68,123,149,230,208,113,243] },
  'C-session':    { crates: ['meerkat-session'], rows: [34,86,225,297,310] },
  'C-auth-prov':  { crates: ['meerkat-auth-core','meerkat-llm-core','meerkat-models'], rows: [8,100,111,75,122,177] },
  'C-tools':      { crates: ['meerkat-tools'], rows: [64,202,299] },
  'C-mcp-mob':    { crates: ['meerkat-mcp-server','meerkat-mob-mcp','meerkat-mob-pack'], rows: [169,157,185,245] },
  'C-facade':     { crates: ['meerkat'], rows: [72,167,337] },
  'C-misc':       { crates: ['meerkat-memory','meerkat-skills','meerkat-hooks','meerkat-cli','meerkat-comms'], rows: [140,279,220,231,170,82,291] },
};

const SCHEMA = {
  type: 'object', additionalProperties: false,
  properties: {
    lane: { type: 'string' },
    summary: { type: 'string' },
    rows: { type: 'array', items: {
      type: 'object', additionalProperties: false,
      properties: {
        id: { type: 'integer' },
        disposition: { type: 'string', enum: ['FIXED','STILL_NEEDS_LEAD','ALREADY_CLOSED','NOT_APPLICABLE'] },
        evidence: { type: 'string' },
        files_changed: { type: 'array', items: { type: 'string' } },
      },
      required: ['id','disposition','evidence'],
    }},
  },
  required: ['lane','rows'],
};

function prompt(lane, cfg) {
  return `You are a Meerkat dogma-remediation IMPLEMENTATION agent. Working dir = the meerkat repo worktree on branch worktree-ultimate-dogma.

Meerkat dogma doctrine: singular typed authority over stringly/Value/JSON; parse-at-boundary into typed enums/newtypes; faults propagated as typed errors, never swallowed or downgraded to String; generated machines own canonical state; surfaces never re-derive truth a seam owns.

YOUR LANE: ${lane}. You may ONLY edit files under: ${cfg.crates.join('; ')}.

A PRIOR verification pass already CONFIRMED each of your rows is genuinely live AND wrote the precise fix design. Read it:
- ${DESIGNS} is a JSON array of {id, lane, evidence}. Find the objects whose "id" is in YOUR row list; the "evidence" field is the verified diagnosis + prescribed fix. IMPLEMENT it.
- ${PLAN} has the full row text (grep "\\*\\*#<id>\\*\\*"): the repair, Owner seam, and Gate.

YOUR ROWS: ${JSON.stringify(cfg.rows)}

FOR EACH ROW:
1. Read its evidence in ${DESIGNS} and its full row in ${PLAN}.
2. Open the owner file(s), confirm the violation is still present, and IMPLEMENT the fix as a write-only edit within your lane's crate(s). Add or update a focused gate test that proves the fix (match the crate's existing test idioms).
3. Classify:
   - FIXED — you implemented the fix entirely within your lane. List files+lines changed and what you did in evidence.
   - STILL_NEEDS_LEAD — implementing it genuinely requires (a) a meerkat-contracts wire-type change + \`make regen-schemas\`, (b) the machine DSL catalog (meerkat-machine-schema) / runtime kernel / \`make machine-codegen\`, or (c) a type change in a crate OUTSIDE your lane. Do the in-lane part you safely can, then describe the precise remaining cross-boundary step in evidence. Do NOT edit other crates.
   - ALREADY_CLOSED — on close inspection the violation is already gone (the prior pass erred). Give concrete file:line evidence.
   - NOT_APPLICABLE — invalid/obsolete row; explain.

ABSOLUTE RULES:
- DO NOT run cargo, make, git, machine-codegen, or regen-schemas. Source edits only; the lead builds, fixes cross-crate cascades, runs gates, and commits.
- Never edit any path containing /generated/, specs/ (*.tla), artifacts/schemas/, meerkat-machine-schema/, or any crate outside your lane.
- No back-compat shims, tombstones, or deprecation aliases — pre-1.0; delete cleanly.
- ABSOLUTELY NO \`.unwrap()\` / \`.expect()\` / \`panic!\` / \`unreachable!\` / \`todo!\` in non-test code (the workspace denies clippy::panic/unwrap_used/expect_used with -D warnings). In tests, prefer \`.expect("msg")\` over panic!. Remove now-unused imports your edit orphans.
- Use \`?\` and typed errors. Match surrounding style, naming, idioms exactly. Keep edits minimal and focused on the row.
- Be rigorous and HONEST. Implement real fixes; never fabricate. If a fix is bigger/riskier than the evidence implies, do the safe core of it and mark STILL_NEEDS_LEAD with the remainder.

Return one entry per assigned row.`;
}

phase('Cascade');
const names = Object.keys(LANES);
log(`WAVE C: ${names.length} lanes, ${Object.values(LANES).reduce((n,c)=>n+c.rows.length,0)} cascade rows`);

const results = await parallel(
  names.map((lane) => () =>
    agent(prompt(lane, LANES[lane]), { label: lane, phase: 'Cascade', schema: SCHEMA })
      .then((r) => r || { lane, rows: [], summary: 'AGENT_RETURNED_NULL' })
  )
);

const all = [];
for (const r of results) if (r && Array.isArray(r.rows)) for (const row of r.rows) all.push({ lane: r.lane, ...row });
const byDisp = {};
for (const x of all) byDisp[x.disposition] = (byDisp[x.disposition] || 0) + 1;
log(`WAVE C dispositions: ${JSON.stringify(byDisp)}`);

return {
  counts: byDisp,
  total: all.length,
  fixed_rows: all.filter((x) => x.disposition === 'FIXED').map((x) => ({ id: x.id, lane: x.lane, files: x.files_changed || [], evidence: x.evidence })),
  still_needs_lead: all.filter((x) => x.disposition === 'STILL_NEEDS_LEAD').map((x) => ({ id: x.id, lane: x.lane, evidence: x.evidence })),
  already_closed_ids: all.filter((x) => x.disposition === 'ALREADY_CLOSED').map((x) => x.id).sort((a, b) => a - b),
  not_applicable: all.filter((x) => x.disposition === 'NOT_APPLICABLE').map((x) => ({ id: x.id, lane: x.lane, evidence: x.evidence })),
  per_lane: results.map((r) => ({ lane: r.lane, n: (r.rows || []).length, summary: r.summary })),
};
