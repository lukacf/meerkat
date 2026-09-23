# 023 — JSON-RPC IDE Integration (TypeScript)

Build IDE extensions and desktop apps with the JSON-RPC interface. JSON-RPC
and REST both use Meerkat's runtime-backed session path; JSON-RPC is convenient
for local stdio or TCP clients and streams events as server notifications.

The connection handshake is inside `try/finally`, so initialization failures
also release the runtime child. Fatal errors exit nonzero. The
[offline regression checks](../003-hello-meerkat-ts/README.md#offline-regression-checks)
exercise success and startup/session failure paths.

## Concepts
- `rkat-rpc` - JSON-RPC 2.0 server over JSONL/stdio or TCP
- Runtime-backed multi-turn session lifecycle
- Capability detection - check features before using them
- Config management - read/write runtime config
- Event notifications are available on the JSON-RPC transport

## Why JSON-RPC over REST?
| Concern | REST | JSON-RPC |
|---------|------|----------|
| Session semantics | Runtime-backed | Runtime-backed |
| Transport | HTTP + JSON | JSONL over stdio or TCP |
| Streaming | Server-sent events | Server notifications |
| Best for | Web apps and services | IDEs, desktop apps, local SDK clients |

## Run
```bash
# From the repository root, first build the local TypeScript SDK and RPC binary:
# npm --prefix sdks/typescript install && npm --prefix sdks/typescript run build
# (cd examples && npm install)
# ./scripts/repo-cargo build -p meerkat-rpc --bin rkat-rpc
# export MEERKAT_BIN_PATH="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rkat-rpc"
ANTHROPIC_API_KEY=sk-... npx tsx examples/023-rpc-ide-integration-ts/main.ts
```
