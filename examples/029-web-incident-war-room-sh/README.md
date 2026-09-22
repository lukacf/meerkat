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
bootstrap assembly. A host application must separately load the generated
`definition.inline.json`, pass it to `@rkat/web` `createMob()`, spawn the declared
members, and add its own prompt and transcript UI before operators can use the
team. Runtime initialization does not expose or instantiate the full packed
definition. Source skills use filesystem paths, which browser member creation
cannot read. The script emits inline copies of the exact markdown alongside
the bundle; merely bootstrapping the mobpack does not convert a caller-supplied
definition's paths.

## Prerequisites
```bash
./scripts/repo-cargo build -p rkat --bin rkat
```

Python 3 is required to emit the inline definition. No provider credentials are
needed for packaging; an integrated host needs credentials for model calls.
All CLI roots are example-local under `.work/`.

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
5. Emits `definition.inline.json` and prints the derived `manifest.web.toml`
6. Prints a realistic incident kickoff prompt from `prompts/incident-kickoff.md`

Generated artifacts land under
`examples/029-web-incident-war-room-sh/.work/incident-war-room-web/`.

## What The Browser Bundle Contains

The browser bundle contains:
- the Meerkat WASM runtime
- the packed mob definition and skills
- `definition.inline.json` for browser `createMob()` (source files remain unchanged)
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

In a custom `@rkat/web` host, load **`definition.inline.json` from the generated
bundle**, not `mobpack/definition.json`. Once the host has initialized its
`MeerkatRuntime`, start with the commander:

```javascript
const response = await fetch("./definition.inline.json");
if (!response.ok) throw new Error(`Definition fetch failed: ${response.status}`);
const definition = await response.json();
const mob = await runtime.createMob(definition);
const profile = definition.orchestrator.profile;
const results = await mob.spawn([{
  profile,
  agent_identity: profile,
}]);
```

`Mob.spawn()` rejects if a member fails to spawn. Successful results carry
`mob_id`, `agent_identity`, and `member_ref`; the SDK does not expose the raw
WASM `status` wrapper.

The same profile/identity shape applies to specialists; full-team wiring and
coordination are a separate host integration step, not exercised by this demo.
Spawning can start model turns; the host owns credentials, lifecycle and teardown.
The inline JSON is a separate host input, not trust-verified by verifying
`mobpack.bin`; serve it with the same integrity controls as the host application.
Then add a prompt input that sends the kickoff scenario from
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

## Offline Regression Test

```bash
node --test --test-force-exit examples/029-web-incident-war-room-sh/test_browser_skills.mjs
```

This covers both 029 and 030 with the current CLI and prebuilt WASM. It compares
every inline skill to the source markdown and spawns each role in its own runtime
using synthetic, in-process provider responses. This proves embedded skill
assembly, not multi-member wiring, live incident reasoning or a finished UI.
Node's `--test-force-exit` closes WASM timer handles after the test verdict;
each fixture explicitly destroys its mob and runtime first.
