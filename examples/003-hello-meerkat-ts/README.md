# 003 — Hello Meerkat (TypeScript SDK)

The simplest TypeScript agent. The SDK spawns `rkat-rpc` as a child process
and can use either an installed/downloaded runtime binary or the repo-local
binary built from this checkout.

## Prerequisites
```bash
# From the repository root when running from a checkout
npm --prefix sdks/typescript install
npm --prefix sdks/typescript run build
(cd examples && npm install)
./scripts/repo-cargo build -p meerkat-rpc --bin rkat-rpc
export MEERKAT_BIN_PATH="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rkat-rpc"
```

## Concepts
- `MeerkatClient` — typed async client
- `connect()` / `close()` — process lifecycle
- `createSession(prompt, options)` — execute a prompt
- `Session` — typed session handle with `.text`, `.usage`, `.id`

## Run
```bash
ANTHROPIC_API_KEY=sk-... npx tsx main.ts
```
