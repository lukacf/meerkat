# 029 — Web Incident War Room (Shell)

Build a browser-deployable mob bundle for a zero-install incident response workspace.

## Why this is cool
- No local install for responders beyond browser access
- Same mob artifact can run in CLI and web
- Useful for incident commanders, SRE triage, and live coordination

## Concepts
- `.mobpack` as universal deployment artifact
- `rkat mob web build` for browser bundle output
- browser-safe capability profile enforced at build time

## Prerequisites
```bash
export ANTHROPIC_API_KEY=sk-...
cargo install rkat wasm-pack
```

## Run
```bash
chmod +x examples.sh && ./examples.sh
```
