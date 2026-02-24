# 028 — Mobpack Release Triage (Shell)

Package a production-ready mob as a single `.mobpack`, validate/sign it, then deploy with explicit trust policy.

## Concepts
- `rkat mob pack` for portable artifact creation
- `rkat mob inspect` and `rkat mob validate` for artifact verification
- `rkat mob deploy` for deterministic run from the same artifact
- strict vs permissive trust policy

## Prerequisites
```bash
export ANTHROPIC_API_KEY=sk-...
cargo install rkat
```

## Run
```bash
chmod +x examples.sh && ./examples.sh
```
