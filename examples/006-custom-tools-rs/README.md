# 006 — Custom Tools (Rust)

Build a travel assistant with weather lookups and unit conversion tools.
Shows the full pattern for tool-augmented agents.

## Concepts
- `AgentToolDispatcher` trait — the tool routing interface
- `ToolDef` — tool name, description, and JSON Schema
- `ToolCallView` — zero-copy view into the LLM's tool call request
- `ToolResult` — success/error response back to the agent loop
- `schemars::JsonSchema` — derive JSON Schema from Rust structs
- `meerkat_tools::schema_for::<T>()` — helper to generate schema

Weather units are a typed enum: `celsius` (the default) or `fahrenheit`.
The emitted JSON Schema and dispatcher both reject unknown units instead of
labeling Celsius values with arbitrary strings. Weather data is simulated.

## Pattern
```
LLM decides to call tool → Agent loop calls dispatch() →
Your code runs → ToolResult returned → Agent loop feeds result back to LLM
```

## Run
```bash
# From the repository root
ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat \
  --example 006-custom-tools --features jsonl-store
```
