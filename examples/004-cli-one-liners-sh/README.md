# 004 — CLI One-Liners (Shell)

Everything you can do with `rkat` from the command line — no code required.

## Concepts
- `rkat run` — single-turn agent execution
- `rkat run --resume last` — multi-turn session resumption
- `rkat session list/show/delete` — session management
- `--isolated` / `--realm` — workspace isolation
- `--verbose` / `--stream` — output modes
- `rkat config` — runtime configuration

## Prerequisites
```bash
export ANTHROPIC_API_KEY=sk-...
./scripts/repo-cargo build -p rkat --bin rkat
```

## Run
```bash
chmod +x examples.sh && ./examples.sh
```
