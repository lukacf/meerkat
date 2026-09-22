# 008 — Structured Output (TypeScript SDK)

Force agents to return JSON matching a specific schema. Build reliable
data pipelines where downstream code can parse agent output safely.

## Concepts
- `outputSchema` — JSON Schema that constrains agent output
- `structuredOutputRetries` — automatic retry on schema validation failure
- Reading the validated value from `session.structuredOutput`
- Pipeline pattern: analyze multiple inputs in sequence

## Why Structured Output?
Without it, LLM output is free-form text. With `outputSchema`, Meerkat
validates the response against your schema, retries on failure, and exposes the
validated value through `session.structuredOutput`.

The confidence field encodes its inclusive 0–1 range with JSON Schema
`minimum` and `maximum`, not just a prose description. Fatal failures exit
nonzero; the runtime is closed even if its initialization handshake fails.

The [offline SDK regression checks](../003-hello-meerkat-ts/README.md#offline-regression-checks)
capture the actual emitted schema and verify rejected and accepted boundary values.

## Schema Format
Standard JSON Schema. Meerkat validates and auto-retries:
```json
{
  "type": "object",
  "properties": { ... },
  "required": ["field1", "field2"]
}
```

## Run
```bash
# From the repository root, first build the local TypeScript SDK and RPC binary:
# npm --prefix sdks/typescript install && npm --prefix sdks/typescript run build
# (cd examples && npm install)
# ./scripts/repo-cargo build -p meerkat-rpc --bin rkat-rpc
# export MEERKAT_BIN_PATH="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rkat-rpc"
ANTHROPIC_API_KEY=sk-... npx tsx examples/008-structured-output-ts/main.ts
```
