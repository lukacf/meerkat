# 029 — Web Incident War Room (Shell)

Build a browser-deployable incident-response workspace that feels like a real
SEV war room, not just a packaging demo. The example produces a browser bundle
containing a multi-role incident team that can coordinate via comms and a
shared task board.

## What This Example Teaches
- No local install for responders beyond browser access
- Same mob artifact can run in CLI and web
- Useful for incident commanders, SRE triage, stakeholder comms, and timeline capture
- Shows how to package a believable multi-agent workflow into a static browser bundle

## Concepts
- `.mobpack` as universal deployment artifact
- `rkat mob web build` for browser bundle output
- browser-safe capability profile enforced at build time
- role-specialized agents using `comms` and `mob_tasks`
- `manifest.web.toml` as derived output that tells you what the browser build can do

## Team Design

The generated incident room includes five roles:

| Role | What it does |
|------|--------------|
| Commander | Owns severity, impact, delegation, and next-step synthesis |
| SRE Lead | Focuses on blast radius, rollback/failover options, and mitigation |
| App Investigator | Tracks deploys, flags, migrations, and likely trigger conditions |
| Customer Comms | Drafts status-page and exec-friendly updates |
| Scribe | Maintains the timeline, decisions, owners, and open questions |

This is intentionally designed to demonstrate why a browser-deployable mob is
useful: you can hand one bundle to an incident commander and get a structured
room with distinct viewpoints, without asking the team to install Rust, Python,
or a local SDK.

## Prerequisites
```bash
export ANTHROPIC_API_KEY=sk-...
cargo install rkat wasm-pack
```

## Run
```bash
./examples.sh
```

## What The Script Does

The script:
1. Copies the source mobpack from `mobpack/` into `.work/`
2. Packs it into `incident-war-room.mobpack`
3. Inspects the artifact so you can see what was bundled
4. Runs `rkat mob web build` to produce a browser bundle
5. Prints the derived `manifest.web.toml`
6. Prints a realistic incident kickoff prompt from `prompts/incident-kickoff.md`

Generated artifacts land under `.work/incident-war-room-web/`.

## What The Browser Bundle Contains

The browser bundle contains:
- the Meerkat WASM runtime
- the packed mob definition and skills
- a derived `manifest.web.toml`
- static assets you can serve with any dumb HTTP server

That means the same incident workflow can be reviewed as source, packed as an
artifact, and then shipped into a zero-install browser environment.

## Serve The Bundle

```bash
cd .work/incident-war-room-web
python3 -m http.server 4173
```

Then open `http://127.0.0.1:4173`.

Bring your own LLM API key in the browser UI when prompted.

## Suggested Exercise

After opening the app, paste the kickoff prompt from
`prompts/incident-kickoff.md`. A good first turn is:

```text
Run this as a SEV-1 war room. State severity and customer impact, create a
task board, assign workstreams to each role, and tell me when the next update
is due.
```

Follow-up turns that make the example feel real:
- "SRE lead: assume rollback is available but will take 12 minutes. Update the plan."
- "Customer comms: draft a 75-word status page update."
- "Scribe: summarize the timeline so far with owners and unresolved risks."
- "Commander: what is the most likely decision point in the next 10 minutes?"

## Files To Read

| File | Purpose |
|------|---------|
| `mobpack/manifest.toml` | Source artifact metadata |
| `mobpack/definition.json` | The war-room team definition used for packing |
| `mobpack/skills/*.md` | Role-specific playbooks that make the team believable |
| `prompts/incident-kickoff.md` | Ready-to-paste scenario prompt for a live drill |
| `examples.sh` | The end-to-end pack → inspect → web-build workflow |
