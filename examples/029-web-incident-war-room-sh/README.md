# 029 - Web Incident War Room (Shell)

Package an incident-response mob and assemble a browser bootstrap bundle. The
generated page trust-verifies the mobpack, initializes the Meerkat WASM
runtime, and exposes a reusable `bootMobpack()` module. It does not create the
mob, spawn its members, or provide a chat UI by itself.

## What This Example Teaches
- How to package one mob artifact for CLI and browser-runtime consumers
- How `rkat mob web build` assembles a static bootstrap from prebuilt WASM
- Which files a host application receives and must integrate
- How to model an incident-response team for a later `@rkat/web` integration

## Concepts
- `.mobpack` as universal deployment artifact
- `rkat mob web build --wasm ...` for browser bundle assembly from a prebuilt runtime
- browser-safe capability profile enforced at build time
- a role-specialized mob definition that requests `comms`
- `manifest.web.toml` as derived output that tells you what the browser build can do

## Team Design

The packed definition declares five roles:

| Role | What it does |
|------|--------------|
| Commander | Owns severity, impact, delegation, and next-step synthesis |
| SRE Lead | Focuses on blast radius, rollback/failover options, and mitigation |
| App Investigator | Tracks deploys, flags, migrations, and likely trigger conditions |
| Customer Comms | Drafts status-page and exec-friendly updates |
| Scribe | Maintains the timeline, decisions, owners, and open questions |

The definition is production-shaped, but this shell example stops at browser
bootstrap assembly. A host application must separately import
`mobpack/definition.json`, inline its referenced skills as described below,
pass the prepared definition to `@rkat/web` `createMob()`, spawn the declared
members, and add its own prompt and transcript UI before operators can use the
team. Runtime initialization does not expose or instantiate the full packed
definition.

### Required browser skill preparation

The source definition uses filesystem-backed `skills` entries, which browser
member construction rejects. Before `createMob()` / `spawn()`, load each
referenced skill's trusted text from `mobpack/skills/*.md` (paths here are
relative to this example directory), or from the trust-verified pack. Replace
each corresponding `definition.skills` entry with
`{ source: "inline", content: skillText }`, preserving the entry's key and every
profile's skill references. Bundle those trusted texts into the host or serve
them as host-managed assets; the WASM runtime cannot read those filesystem paths.

`initFromMobpack()` compiles verified pack skills into standalone session
prompts. It does not supply filesystem skills to a separately imported
`MobDefinition`; that definition still needs the explicit inlining step.

## Prerequisites
```bash
export ANTHROPIC_API_KEY=sk-...
./scripts/repo-cargo build -p rkat --bin rkat
```

The script uses `sdks/web/wasm/meerkat_web_runtime_bg.wasm` by default. To
rebuild that artifact after Rust changes, install Node.js and `wasm-pack`, then
run:

```bash
npm --prefix sdks/web install
npm --prefix sdks/web run build:wasm
```

Set `MEERKAT_WASM=/path/to/meerkat_web_runtime_bg.wasm` to use another
prebuilt runtime.

If you are running from this repo checkout instead of a global install, the
script will automatically prefer repo-local binaries built by
`./scripts/repo-cargo` when present.

## Run
```bash
./examples/029-web-incident-war-room-sh/examples.sh
```

## What The Script Does

The script:
1. Copies the source mobpack from `mobpack/` into `.work/`
2. Packs it into `incident-war-room.mobpack`
3. Inspects the artifact so you can see what was bundled
4. Runs `rkat mob web build --wasm ...` to assemble a browser bundle
5. Prints the derived `manifest.web.toml`
6. Prints a realistic incident kickoff prompt from `prompts/incident-kickoff.md`

Generated artifacts land under
`examples/029-web-incident-war-room-sh/.work/incident-war-room-web/`.

## What The Browser Bundle Contains

The browser bundle contains:
- the Meerkat WASM runtime
- the packed mob definition and skills
- a derived `manifest.web.toml`
- static assets you can serve with any dumb HTTP server

The generated `index.html` is an initialization smoke page. Its Start button
loads the WASM module and trust-verifies `mobpack.bin`; successful status does
not mean the declared team has been instantiated.

## Serve The Bundle

```bash
cd examples/029-web-incident-war-room-sh/.work/incident-war-room-web
python3 -m http.server 4173
```

Then open `http://127.0.0.1:4173`.

Enter an API key to exercise runtime initialization. Provider calls do not run
until a host application creates sessions or mob members.

## Suggested Integration Exercise

In a custom `@rkat/web` host, import `mobpack/definition.json`, replace its
referenced path skills with inline trusted text as above, then create the mob
and spawn its members. Add a prompt input that sends the kickoff scenario from
`prompts/incident-kickoff.md` to the commander. A good first turn is:

```text
Run this as a SEV-1 war room. State severity and customer impact, assign
workstreams to each role, and tell me when the next update is due.
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
