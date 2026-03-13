# 030 — Web Dashboard Copilot (Shell)

Build a browser-deployable **release command center copilot** meant to live
inside an internal ops dashboard next to rollout controls, latency charts, and
incident notes.

This example is intentionally shell-driven, but it is no longer just an
artifact recipe. It produces a believable multi-agent bundle, validates it,
builds the browser runtime, and emits the supporting assets you would actually
want when embedding the result into a product.

## What This Example Teaches

- **Embeddable browser bundle** built from a portable `.mobpack`
- **A real dashboard workflow**: rollout triage, metrics analysis, rollback
  recommendation, and status drafting
- **Multi-agent specialization** inside a web-safe mob
- **Derived web deployment metadata** via `manifest.web.toml`
- **Pedagogical embedding assets**: sample dashboard state, example prompts,
  and a starter iframe snippet

## The Team Inside The Copilot

The generated mob has four roles:

| Role | Purpose |
|------|---------|
| `incident-commander` | User-facing copilot that coordinates the team and gives the operator a verdict |
| `metrics-analyst` | Interprets latency, error-rate, queue-depth, and regional health signals |
| `rollout-guard` | Decides whether to continue, pause, or roll back based on blast radius and safeguards |
| `status-scribe` | Produces Slack-ready status updates and next-step checklists |

This is the kind of team you would actually embed in a release dashboard:
specialists reason separately, then the user-facing copilot turns that into a
clear operator recommendation.

## Prerequisites

```bash
export ANTHROPIC_API_KEY=sk-...
cargo install rkat wasm-pack
```

If you are working from this repo checkout instead of a global install, the
script will prefer `../../target/debug/rkat` automatically when it exists.

## Run

```bash
./examples.sh
```

## What The Script Does

The script will:

1. Generate a realistic mobpack source tree under `.work/dashboard-copilot/`
2. Create a portable `.mobpack`
3. Run `rkat mob inspect` and `rkat mob validate`
4. Build the browser bundle with `rkat mob web build`
5. Emit supporting assets:
   - `.work/dashboard-context.json`
   - `.work/example-questions.md`
   - `.work/embed-snippet.html`

## What The Browser Bundle Contains

The generated `.work/dashboard-copilot-web/` directory is the embeddable
browser artifact. It includes:

- the WASM runtime files,
- the generated `manifest.web.toml`,
- the packed mob definition and skills prepared for web execution.

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

This copilot is meant to sit beside those widgets and answer questions like:

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

A minimal iframe-based integration sketch for teams evaluating how the web
bundle would slot into an existing dashboard shell.

## Suggested Host Integration Pattern

1. Build the web bundle with this script
2. Serve `.work/dashboard-copilot-web/` from your dashboard host
3. Render it inside an iframe or side panel
4. Inject current dashboard context from your host application
5. Let operators ask release/incident questions against that live state

## Notes

- This example focuses on **packaging and deployment ergonomics**, not on a
  polished custom frontend. The browser UI comes from the standard Meerkat web
  build surface.
- `rkat mob web build` shells out to `wasm-pack`, so `wasm-pack` must be
  installed and available on `PATH`.
- The generated assets under `.work/` are intentionally inspectable so you can
  understand what would be embedded and why.
