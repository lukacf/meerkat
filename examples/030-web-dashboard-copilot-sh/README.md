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
The host still has to load the generated browser `definition.inline.json`, use
`@rkat/web` to create the mob and spawn the specialists, route prompts, and
render results. Runtime initialization does not expose or instantiate the full
packed definition. Do not pass the source definition's filesystem skill paths
to `createMob()`: WASM cannot read them. The script emits exact inline markdown
copies while preserving the path-based source and packaged files.

## Prerequisites

```bash
./scripts/repo-cargo build -p rkat --bin rkat
```

Python 3 is required to emit the inline definition. Packaging does not require
provider credentials; actual model calls in a host application do. All CLI
roots are example-local under `.work/`.

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
   - `.work/dashboard-copilot-web/definition.inline.json`
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
- `definition.inline.json` with browser-readable inline skills for `createMob()`.

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

After a host application loads `definition.inline.json`, creates the mob, and
connects a prompt UI, the copilot could sit beside those widgets and answer
questions like:

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
2. Load `definition.inline.json` from the generated bundle and pass it to
   `@rkat/web` `runtime.createMob()`; then spawn the declared profiles. The
   [029 integration snippet](../029-web-incident-war-room-sh/README.md#suggested-integration-exercise)
   works unchanged with this definition. Runtime bootstrap alone does not
   convert path skills in a caller-supplied definition.
3. Add prompt, event, and transcript UI for the operator
4. Pass current dashboard context into that host application
5. Serve or embed the completed host experience in the dashboard

The inline definition is a separate host input, not covered by verification of
`mobpack.bin`; protect it with the same integrity controls as your host assets.
Spawning can start model turns; the host owns credentials and teardown.

## Offline Regression Test

```bash
node --test --test-force-exit examples/029-web-incident-war-room-sh/test_browser_skills.mjs
```

This covers both web examples using the current CLI and actual prebuilt WASM,
with synthetic provider responses and no external requests. Roles are tested
individually; this is not a multi-member wiring, live-provider or finished-UI test.
The force-exit flag closes lingering WASM timers after explicit fixture teardown
and the test verdict.

## Notes

- This example focuses on **packaging and bootstrap ergonomics**, not on a
  functional custom frontend. The standard generated page is an initialization
  smoke surface.
- `rkat mob web build` copies a prebuilt WASM artifact; it does not invoke
  `wasm-pack`.
- The generated assets under `.work/` are intentionally inspectable so you can
  understand what would be embedded and why.
