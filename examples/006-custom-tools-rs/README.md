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

## Pattern
```
LLM decides to call tool → Agent loop calls dispatch() →
Your code runs → ToolResult returned → Agent loop feeds result back to LLM
```

## Run
```bash
# This is a reference implementation. For runnable examples, see meerkat/examples/.
```
