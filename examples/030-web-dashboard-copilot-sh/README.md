# 030 — Web Dashboard Copilot (Shell)

Create a browser bundle meant to be embedded in an internal dashboard (logs, deploy health, rollout checklists).

## Why this is cool
- Drop-in assistant panel in an existing web app
- Great for release operations and live observability workflows
- Same pack can be promoted across environments

## Concepts
- build once (`.mobpack`), deploy many (CLI + browser)
- `manifest.web.toml` is generated output, not authored input
- explicit capability policy for browser-safe execution

## Prerequisites
```bash
export ANTHROPIC_API_KEY=sk-...
cargo install rkat wasm-pack
```

## Run
```bash
chmod +x examples.sh && ./examples.sh
```
