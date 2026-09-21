# 030 - Web Dashboard Copilot (Shell)

Package the definition and bootstrap assets for a browser-hosted **release
command center copilot** meant to live inside an internal ops dashboard next
to rollout controls, latency charts, and incident notes.

This shell-driven example produces a production-shaped multi-agent artifact,
validates it, assembles a browser runtime bootstrap, and emits supporting assets
for a future host integration. The generated page initializes the runtime; it
does not instantiate the mob or provide a dashboard copilot UI by itself.

## What This Example Teaches

- **Browser bootstrap bundle** built from a portable `.mobpack`
- **A dashboard-oriented mob definition** for rollout triage, metrics analysis,
  rollback recommendations, and status drafting
- **Multi-agent specialization** described by a web-safe artifact
- **Derived web deployment metadata** via `manifest.web.toml`
- **Pedagogical embedding assets**: sample dashboard state, example prompts,
  and a starter iframe snippet

## The Team Inside The Copilot

The generated definition has four roles:

| Role | Purpose |
|------|---------|
| `incident-commander` | User-facing copilot that coordinates the team and gives the operator a verdict |
| `metrics-analyst` | Interprets latency, error-rate, queue-depth, and regional health signals |
| `rollout-guard` | Decides whether to continue, pause, or roll back based on blast radius and safeguards |
| `status-scribe` | Produces Slack-ready status updates and next-step checklists |

This is the kind of team a host application could embed in a release dashboard.
The host still has to import the generated source
`.work/dashboard-copilot/definition.json`, inline its referenced skills as
described below, use `@rkat/web` to create the mob and spawn the specialists,
route prompts, and render results. Runtime initialization does not expose or
instantiate the full packed definition.

### Required browser skill preparation

The generated definition uses filesystem-backed `skills` entries, which browser
member construction rejects. Before `createMob()` / `spawn()`, load each
referenced skill's trusted text from `.work/dashboard-copilot/skills/*.md`
(paths here are relative to this example directory), or from the trust-verified
pack. Replace each corresponding `definition.skills` entry with
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
rebuild it after Rust changes, install Node.js and `wasm-pack`, then run:

```bash
npm --prefix sdks/web install
npm --prefix sdks/web run build:wasm
```

Set `MEERKAT_WASM=/path/to/meerkat_web_runtime_bg.wasm` to select a different
prebuilt runtime.

If you are working from this repo checkout instead of a global install, the
script will prefer repo-local binaries built by `./scripts/repo-cargo` when
present.

## Run

```bash
./examples/030-web-dashboard-copilot-sh/examples.sh
```

## What The Script Does

The script will:

1. Generate a realistic mobpack source tree under `.work/dashboard-copilot/`
2. Create a portable `.mobpack`
3. Run `rkat mob inspect` and `rkat mob validate`
4. Assemble the browser bundle with `rkat mob web build --wasm ...`
5. Emit supporting assets:
   - `.work/dashboard-context.json`
   - `.work/example-questions.md`
   - `.work/embed-snippet.html`

These `.work` paths are relative to
`examples/030-web-dashboard-copilot-sh/`.

## What The Browser Bundle Contains

The generated `.work/dashboard-copilot-web/` directory is a browser bootstrap
artifact. It includes:

- the WASM runtime files,
- the generated `manifest.web.toml`,
- the packed mob definition and skills prepared for web execution.

Its generated `index.html` only trust-verifies the pack and initializes the
WASM runtime. The Start button does not create or spawn the declared team.

`manifest.web.toml` is **derived output**, not a source file you hand-author.
It tells you what web-safe contract the build surface produced from the source
mobpack.

## Suggested Usage

Imagine your internal release dashboard already shows:

- current rollout percentage,
- p95 latency and error-rate charts,
- regional health,
- queue depth,
- the last 20 minutes of operator notes.

After a host application inlines the skills, creates the mob, and connects a
prompt UI, the copilot could sit beside those widgets and answer questions like:

- "Do we continue the rollout or pause it?"
- "Which metric moved first after the deployment?"
- "Is this degradation localized or systemic?"
- "Write me a concise incident-channel update."
- "What is the blast radius if we wait 10 more minutes?"

That is a much more realistic embedding story than a generic "assistant panel."

## Generated Assets

The example intentionally emits files that make the embedding story concrete:

### `.work/dashboard-context.json`

A sample rollout snapshot for `checkout-api`, including baseline metrics,
current degraded signals, regional health, and operator notes. Use it as the
kind of context your dashboard host would pass into the embedded experience.

### `.work/example-questions.md`

Starter prompts that show what the copilot is for.

### `.work/embed-snippet.html`

A minimal iframe placement sketch. It embeds the generated initialization page,
not a finished copilot panel.

## Suggested Host Integration Pattern

1. Build the web bootstrap with this script
2. Import `.work/dashboard-copilot/definition.json` and replace its referenced
   path skills with inline trusted text from `.work/dashboard-copilot/skills/`,
   retaining the skill keys and profile references
3. Use `@rkat/web` in a host application to create the prepared mob and spawn its members
4. Add prompt, event, and transcript UI for the operator
5. Pass current dashboard context into that host application
6. Serve or embed the completed host experience in the dashboard

## Notes

- This example focuses on **packaging and bootstrap ergonomics**, not on a
  functional custom frontend. The standard generated page is an initialization
  smoke surface.
- `rkat mob web build` copies a prebuilt WASM artifact; it does not invoke
  `wasm-pack`.
- The generated assets under `.work/` are intentionally inspectable so you can
  understand what would be embedded and why.
