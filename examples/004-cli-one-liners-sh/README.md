# 004 — CLI One-Liners (Shell)

Everything you can do with `rkat` from the command line — no code required.

## Concepts
- `rkat run` — single-turn agent execution
- `rkat resume` — multi-turn session resumption
- `rkat sessions list/read/archive` — session management
- `--isolated` / `--realm` — workspace isolation
- `--verbose` / `--stream` — output modes
- `rkat config` — runtime configuration

## Prerequisites
```bash
export ANTHROPIC_API_KEY=sk-...
cargo build -p meerkat-cli
```

## Run
```bash
chmod +x examples.sh && ./examples.sh
```
