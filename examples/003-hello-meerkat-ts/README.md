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
ANTHROPIC_API_KEY=sk-... npx tsx examples/003-hello-meerkat-ts/main.ts
```

The cleanup scope includes the connection handshake, so failed initialization
also closes the spawned runtime. Fatal errors print a diagnostic and exit nonzero.

## Offline regression checks
After installing the example dependencies and building the local TypeScript SDK,
run `npm --prefix examples run check:sdk` and
`npm --prefix examples run test:sdk` from the repository root. The latter also
needs Python 3.10+ and the local Python SDK's dependencies; it runs the nine SDK
examples with local JSONL/HTTP fixtures and validates the emitted sentiment schema
with Ajv. It makes no provider calls and does not replace live-provider validation.
