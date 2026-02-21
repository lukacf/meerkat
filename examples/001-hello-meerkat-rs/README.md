# 001 — Hello Meerkat (Rust)

The simplest possible Meerkat agent. Send one prompt, get one response.

## Concepts
- `AgentFactory` — shared wiring for agent components
- `AgentBuilder` — fluent API for constructing agents
- `AnthropicClient` — LLM provider implementation
- `RunResult` — structured output from an agent turn

## Run
```bash
ANTHROPIC_API_KEY=sk-... cargo run --example 001_hello_meerkat
```
