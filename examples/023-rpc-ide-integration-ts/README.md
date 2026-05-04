# 023 — JSON-RPC IDE Integration (TypeScript)

Build IDE extensions and desktop apps with the JSON-RPC interface. Agents
stay alive between turns for instant multi-turn conversations.

## Concepts
- `rkat-rpc` — JSON-RPC 2.0 server over JSONL/stdio
- `SessionRuntime` — keeps agents alive between turns (no reconstruction)
- Capability detection — check features before using them
- Config management — read/write runtime config
- Event notifications are available on the JSON-RPC transport

## Why JSON-RPC over REST?
| Feature | REST | JSON-RPC |
|---------|------|----------|
| Agent lifetime | Per-request | Kept alive |
| Turn latency | Agent reconstruction | Instant |
| Streaming | SSE setup | Stdio notifications |
| Best for | Web apps, microservices | IDEs, desktop apps |

## Run
```bash
# From the repository root, first build the local TypeScript SDK and RPC binary:
# npm --prefix sdks/typescript install && npm --prefix sdks/typescript run build
# (cd examples && npm install)
# ./scripts/repo-cargo build -p meerkat-rpc --bin rkat-rpc
# export MEERKAT_BIN_PATH="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rkat-rpc"
ANTHROPIC_API_KEY=sk-... npx tsx main.ts
```
