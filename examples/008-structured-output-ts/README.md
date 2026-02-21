# 008 — Structured Output (TypeScript SDK)

Force agents to return JSON matching a specific schema. Build reliable
data pipelines where downstream code can parse agent output safely.

## Concepts
- `output_schema` — JSON Schema that constrains agent output
- `structured_output_retries` — automatic retry on schema validation failure
- Typed parsing of structured results
- Pipeline pattern: analyze multiple inputs in sequence

## Why Structured Output?
Without it, LLM output is free-form text. With `output_schema`, Meerkat
validates the response against your schema and retries on failure.

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
ANTHROPIC_API_KEY=sk-... npx tsx main.ts
```
