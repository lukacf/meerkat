export const meta = {
  name: 'dogma-357-verify-fix',
  description: 'Verify 357 dogma rows vs current HEAD per crate-lane; fix genuinely-live mechanical rows write-only; flag design-heavy/regen rows for the lead',
  phases: [
    { title: 'VerifyFix', detail: 'one agent per crate-lane: verify vs HEAD, fix mechanical, flag design-heavy' },
  ],
};

const PLAN = '/Users/luka/src/meerkat/.claude/worktrees/dogma-2/docs/architecture/dogma-audits/REMEDIATION-PLAN-357.md';

// lane -> { crates: [dir...], rows: [id...], fix: bool }
const LANES = {
  'A-comms':          { crates: ['meerkat-comms'], rows: [58,82,90,241,242,267,268,269,291,292,300,344], fix: true },
  'A-core':           { crates: ['meerkat-core'], rows: [5,6,10,12,30,31,44,57,65,66,76,84,92,93,102,119,128,129,130,143,145,146,147,151,152,166,194,195,205,207,220,228,233,239,253,270,275,279,308,309,312,322,325,331,332,340,356], fix: true },
  'A-facade':         { crates: ['meerkat'], rows: [46,51,52,68,69,72,91,103,107,108,109,111,123,124,132,134,141,142,149,150,153,167,179,180,196,208,218,223,230,236,237,243,271,272,273,274,276,281,286,301,319,323,324,326,329,337,346,355], fix: true },
  'A-mcp-mobpack':    { crates: ['meerkat-mcp-server','meerkat-mcp','meerkat-mob-mcp','meerkat-mob-pack'], rows: [18,37,74,95,112,133,163,169,185,189,200,211,229], fix: true },
  'A-mob':            { crates: ['meerkat-mob'], rows: [13,14,53,70,99,125,126,155,181,210,258,259,260,261,262,282,293,303,304,314,320,328,333,334,336,347], fix: true },
  'A-providers':      { crates: ['meerkat-auth-core','meerkat-llm-core','meerkat-openai','meerkat-anthropic','meerkat-gemini','meerkat-live','meerkat-models','meerkat-schedule'], rows: [7,8,40,43,47,48,49,60,73,75,80,81,100,113,121,122,148,177,178,254,278,349], fix: true },
  'A-rest-cli':       { crates: ['meerkat-rest','meerkat-cli'], rows: [29,170,226,244,245,317,342], fix: true },
  'A-rpc':            { crates: ['meerkat-rpc'], rows: [61,62,67,85,97,98,105,115,156,157,176,184,191,198,199,209,213,214,249,302,338,348], fix: true },
  'A-runtime':        { crates: ['meerkat-runtime'], rows: [2,3,24,25,27,28,33,41,45,56,106,110,154,182,183,206,263,277,287,298,341,350], fix: true },
  'A-session':        { crates: ['meerkat-session'], rows: [1,4,34,71,86,120,131,144,164,225,265,297,310], fix: true },
  'A-skills-hooks':   { crates: ['meerkat-skills','meerkat-hooks'], rows: [11,35,36,42,77,165,219,231,280,289,290,311], fix: true },
  'A-store-memory':   { crates: ['meerkat-store','meerkat-memory'], rows: [50,104,140,232,238,240,288,335], fix: true },
  'A-tools':          { crates: ['meerkat-tools'], rows: [63,64,78,79,201,202,252,299,318,352], fix: true },
  'A-webrt-workgraph':{ crates: ['meerkat-web-runtime','meerkat-workgraph'], rows: [116,117,118,137], fix: true },
  'V-contracts':      { crates: ['meerkat-contracts'], rows: [9,15,16,26,32,54,59,83,87,94,96,101,135,158,159,160,190,193,197,204,234,235,255,256,257,305,313,321,339,343], fix: false },
  'V-sdks':           { crates: ['sdks','tools/sdk-codegen'], rows: [38,39,55,88,89,127,136,138,161,173,186,192,215,216,217,222,246,247,248,283,306,330,353,357], fix: false },
  'V-ci':             { crates: ['xtask','scripts'], rows: [17,19,20,22,23,162,175,203,221,251,266,285,296,327], fix: false },
};

const SCHEMA = {
  type: 'object', additionalProperties: false,
  properties: {
    lane: { type: 'string' },
    summary: { type: 'string' },
    rows: {
      type: 'array',
      items: {
        type: 'object', additionalProperties: false,
        properties: {
          id: { type: 'integer' },
          disposition: { type: 'string', enum: ['FIXED','ALREADY_CLOSED','LIVE_NEEDS_LEAD','NOT_APPLICABLE'] },
          evidence: { type: 'string' },
          files_changed: { type: 'array', items: { type: 'string' } },
        },
        required: ['id','disposition','evidence'],
      },
    },
  },
  required: ['lane','rows'],
};

function prompt(lane, cfg) {
  const crateDirs = cfg.crates.join(', ');
  const editClause = cfg.fix
    ? `You MAY apply write-only fixes, but ONLY to files under: ${crateDirs}.`
    : `This is a VERIFY-ONLY lane (the fixes here need schema/SDK regen or CI-gate work the lead owns). DO NOT edit any files. Classify each row only.`;
  return `You are a Meerkat dogma-remediation agent. The working directory is the meerkat repo worktree on branch worktree-ultimate-dogma.

Meerkat dogma doctrine (what a "violation" is): singular typed authority over stringly/Value/JSON; parse-at-boundary into typed enums/newtypes; faults propagated as typed errors, never swallowed or downgraded to a String; generated state machines own canonical semantic state; surfaces never re-derive truth a seam already owns.

YOUR LANE: ${lane}. Owned crate dir(s): ${crateDirs}.
${editClause}

THE REMEDIATION PLAN (357 rows) is at:
${PLAN}
Each row reads: \`- **#<id>** <name> — *[severity · effort · status]* — <repair>. **Owner:** <seam>. **Gate:** <verification>.\`
Read individual rows with: grep -n "\\*\\*#<id>\\*\\*" "${PLAN}" (then read surrounding lines).

ROWS ASSIGNED TO YOU: ${JSON.stringify(cfg.rows)}

CRITICAL: the plan's \`status\` field is STALE (baseline a1ec8de86). This branch has ALREADY CLOSED ~110-130 rows since then via prior commits. So you MUST verify each row's CURRENT state in this worktree — do NOT trust the plan's "live" label. Many will already be closed.

FOR EACH ASSIGNED ROW:
1. Read the row in the plan. Note the OLD/violating pattern, the prescribed repair, the owner file, and the gate.
2. Open the owner file(s) in THIS worktree and read the CURRENT code. Grep for both the OLD pattern (is it gone?) and the typed owner the repair prescribes (does it exist?).
3. Classify into exactly one disposition:
   - ALREADY_CLOSED — the violating pattern is gone and a typed owner / fail-closed path is present. Provide CONCRETE evidence: file:line + the actual current typed construct. Cite real code, never a guess.
   - FIXED — the row is genuinely still live AND the fix is mechanical and ENTIRELY inside your owned crate(s): no machine-codegen, no meerkat-contracts wire-type change, no schema/SDK regen, no type change that cascades outside your crate. ${cfg.fix ? 'Apply the fix as a write-only edit.' : 'NOT ALLOWED in this verify-only lane — use LIVE_NEEDS_LEAD instead.'} List files+lines changed and what you did in evidence.
   - LIVE_NEEDS_LEAD — genuinely live BUT the fix needs: (a) the machine DSL catalog (meerkat-machine-schema) or runtime kernel dsl.rs or \`make machine-codegen\`, OR (b) a meerkat-contracts wire-type change + \`make regen-schemas\`, OR (c) a cross-crate type change cascading outside your lane. Describe the precise fix (files, the typed owner to introduce, the cascade) in evidence. DO NOT edit.
   - NOT_APPLICABLE — the row is invalid/obsolete/"retire_invalid"; explain why in evidence.

ABSOLUTE RULES:
- DO NOT run cargo, make, git, machine-codegen, or regen-schemas. You produce source edits only; the lead builds, fixes cross-crate cascades, and commits.
- Never edit any path containing \`/generated/\`, \`specs/\` (*.tla), \`artifacts/schemas/\`, \`meerkat-machine-schema/\`, or any crate outside your lane.
- No back-compat shims, no tombstones, no deprecation aliases — this repo is pre-1.0; delete cleanly.
- No \`.unwrap()\` / \`.expect()\` / \`panic!\` in non-test code; use \`?\` and typed errors.
- Match the surrounding code's style, naming, and idioms exactly.
- PREFER ALREADY_CLOSED: the typed owners for most rows already exist from prior work. Verify honestly — only mark FIXED when you actually changed live code; only mark LIVE_* with concrete evidence the violation persists.
- Be rigorous and HONEST. Never fabricate evidence. If you cannot determine state confidently after reading the code, mark LIVE_NEEDS_LEAD and state exactly what you checked and what is uncertain.

Return one entry per assigned row in the structured result.`;
}

phase('VerifyFix');
const laneNames = Object.keys(LANES);
log(`dogma-357 verify+fix: ${laneNames.length} lanes over ${Object.values(LANES).reduce((n,c)=>n+c.rows.length,0)} rows`);

const results = await parallel(
  laneNames.map((lane) => () =>
    agent(prompt(lane, LANES[lane]), { label: lane, phase: 'VerifyFix', schema: SCHEMA })
      .then((r) => r || { lane, rows: [], summary: 'AGENT_RETURNED_NULL' })
  )
);

// Aggregate dispositions
const all = [];
for (const r of results) {
  if (!r || !Array.isArray(r.rows)) continue;
  for (const row of r.rows) all.push({ lane: r.lane, ...row });
}
const byDisp = {};
for (const x of all) byDisp[x.disposition] = (byDisp[x.disposition] || 0) + 1;
log(`dispositions: ${JSON.stringify(byDisp)} (total ${all.length})`);

const fixed = all.filter((x) => x.disposition === 'FIXED');
const needsLead = all.filter((x) => x.disposition === 'LIVE_NEEDS_LEAD');
const closed = all.filter((x) => x.disposition === 'ALREADY_CLOSED');
const na = all.filter((x) => x.disposition === 'NOT_APPLICABLE');

return {
  counts: byDisp,
  total: all.length,
  fixed_rows: fixed.map((x) => ({ id: x.id, lane: x.lane, files: x.files_changed || [], evidence: x.evidence })),
  needs_lead_rows: needsLead.map((x) => ({ id: x.id, lane: x.lane, evidence: x.evidence })),
  already_closed_ids: closed.map((x) => x.id).sort((a, b) => a - b),
  not_applicable_rows: na.map((x) => ({ id: x.id, lane: x.lane, evidence: x.evidence })),
  per_lane: results.map((r) => ({ lane: r.lane, summary: r.summary, n: (r.rows || []).length })),
};
