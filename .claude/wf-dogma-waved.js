export const meta = {
  name: 'dogma-357-waved-contracts',
  description: 'Introduce/tighten typed wire structs in meerkat-contracts/wire per-module (file-disjoint); update in-contracts consumers; flag surface/SDK/regen cascade for the lead',
  phases: [{ title: 'Contracts', detail: 'one agent per wire/ module: typed wire structs + in-contracts consumers' }],
};

const PLAN = '/Users/luka/src/meerkat/.claude/worktrees/dogma-2/docs/architecture/dogma-audits/REMEDIATION-PLAN-357.md';
const DESIGNS = '/Users/luka/src/meerkat/.claude/worktrees/ultimate-dogma/.claude/dogma-needs-lead.json';

const LANES = {
  'D-mob':     { files: ['meerkat-contracts/src/wire/mob.rs'], rows: [26,54,59,135,159,190,235,313] },
  'D-comms':   { files: ['meerkat-contracts/src/wire/comms.rs'], rows: [94,96,101] },
  'D-session': { files: ['meerkat-contracts/src/wire/session.rs'], rows: [87] },
  'D-conn':    { files: ['meerkat-contracts/src/wire/connection.rs'], rows: [9] },
  'D-live-supbridge': { files: ['meerkat-contracts/src/wire/live.rs','meerkat-contracts/src/wire/supervisor_bridge.rs'], rows: [257,343] },
  'D-meta':    { files: ['meerkat-contracts/src/emit.rs','meerkat-contracts/src/rpc_catalog.rs'], rows: [158,234] },
  'D-models':  { files: ['meerkat-contracts/src/wire/models.rs'], rows: [339] },
  'D-skills':  { files: ['meerkat-contracts/src/wire/skills.rs'], rows: [15] },
};

const SCHEMA = {
  type: 'object', additionalProperties: false,
  properties: {
    lane: { type: 'string' }, summary: { type: 'string' },
    rows: { type: 'array', items: {
      type: 'object', additionalProperties: false,
      properties: {
        id: { type: 'integer' },
        disposition: { type: 'string', enum: ['TYPED','STILL_NEEDS_LEAD','ALREADY_CLOSED','NOT_APPLICABLE'] },
        evidence: { type: 'string' },
        cascade: { type: 'string', description: 'out-of-contracts consumers (rpc/mob/rest/sdk) + regen the lead must do' },
        files_changed: { type: 'array', items: { type: 'string' } },
      },
      required: ['id','disposition','evidence'],
    }},
  },
  required: ['lane','rows'],
};

function prompt(lane, cfg) {
  return `You are a Meerkat dogma WIRE-CONTRACT agent on branch worktree-ultimate-dogma.

meerkat-contracts is the single source of truth for wire types; SDK types + JSON schemas are GENERATED from it via \`make regen-schemas\`. Dogma: no \`serde_json::Value\` bags or bare \`String\`/\`Option<String>\` where the shape is known — define typed structs/enums; parse-at-boundary; \`into_core_request\`/\`From\` impls validate into typed core.

YOUR LANE: ${lane}. Edit ONLY these contracts files: ${cfg.files.join(', ')}.

Designs: ${DESIGNS} (JSON array of {id, lane, evidence}); find your ids, "evidence" is the verified diagnosis. Full row: grep "\\*\\*#<id>\\*\\*" "${PLAN}".

YOUR ROWS: ${JSON.stringify(cfg.rows)}

FOR EACH ROW: read the evidence + the current wire type. Introduce/tighten the typed wire struct or enum that replaces the Value/String bag (define it in your wire module). Update the IN-CONTRACTS consumers you own: \`into_core_request\`, \`From\`/\`TryFrom\` impls, \`#[cfg_attr(feature="schema", derive(JsonSchema))]\`, serde attrs, and any in-module round-trip. Keep \`#[serde(...)]\` + schema derives so \`regen-schemas\` emits the new shape. Classify:
   - TYPED — you defined/tightened the typed wire shape + updated in-contracts consumers. In "cascade", LIST precisely the out-of-contracts work the lead must do: which surface producers/parsers (meerkat-rpc/mob/rest), which SDK helpers, and that \`make regen-schemas\` + version-parity must run. List files+lines you changed.
   - STILL_NEEDS_LEAD — the typed owner genuinely cannot be defined in-contracts without a core type first; describe it.
   - ALREADY_CLOSED — the typed shape already exists + is used (cite file:line).
   - NOT_APPLICABLE — invalid/obsolete.

ABSOLUTE RULES:
- DO NOT run cargo/make/git/regen-schemas. Source edits only; the lead runs regen, fixes surface+SDK consumers, and commits.
- Edit ONLY your lane's contracts files. NEVER edit generated/, artifacts/schemas/, sdks/, or surface crates — describe those in "cascade".
- Preserve schemars/serde derives so the type is schema-emittable. No back-compat shims. No .unwrap/.expect/panic in non-test code.
- The wire type may need a matching core domain type; if it already exists, reference it; if not, keep the wire type self-contained + note the core gap in cascade.
- Be HONEST + rigorous. Define real typed shapes, not Value-with-a-newtype-wrapper.

Return one entry per assigned row.`;
}

phase('Contracts');
const names = Object.keys(LANES);
log(`WAVE D: ${names.length} wire-module lanes, ${Object.values(LANES).reduce((n,c)=>n+c.rows.length,0)} contract rows`);
const results = await parallel(names.map((lane) => () =>
  agent(prompt(lane, LANES[lane]), { label: lane, phase: 'Contracts', schema: SCHEMA })
    .then((r) => r || { lane, rows: [], summary: 'AGENT_RETURNED_NULL' })));
const all = [];
for (const r of results) if (r && Array.isArray(r.rows)) for (const row of r.rows) all.push({ lane: r.lane, ...row });
const byDisp = {};
for (const x of all) byDisp[x.disposition] = (byDisp[x.disposition] || 0) + 1;
log(`WAVE D dispositions: ${JSON.stringify(byDisp)}`);
return {
  counts: byDisp, total: all.length,
  typed_rows: all.filter((x) => x.disposition === 'TYPED').map((x) => ({ id: x.id, lane: x.lane, files: x.files_changed || [], cascade: x.cascade || '', evidence: x.evidence })),
  still_needs_lead: all.filter((x) => x.disposition === 'STILL_NEEDS_LEAD').map((x) => ({ id: x.id, lane: x.lane, evidence: x.evidence })),
  already_closed_ids: all.filter((x) => x.disposition === 'ALREADY_CLOSED').map((x) => x.id).sort((a,b)=>a-b),
  not_applicable: all.filter((x) => x.disposition === 'NOT_APPLICABLE').map((x) => ({ id: x.id, lane: x.lane, evidence: x.evidence })),
};
