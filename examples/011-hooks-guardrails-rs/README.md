# 011 — Hooks & Guardrails (Rust)

Intercept and control agent behavior at 8 defined hook points. Use hooks for
audit logging, content filtering, approval gates, cost tracking, and more.

## Concepts
- `HookPoint` — 8 interception points in the agent loop
- `HookCapability` — observe (read-only) or guardrail (Allow/Deny)
- `HookExecutionMode` — foreground (blocking) or background (async)
- `HookAdapterConfig` — command, HTTP, or in-process execution
- `DefaultHookEngine` — the standard hook processor

## Hook Points
```
pre_llm_request → LLM → post_llm_response → pre_tool_execution → Tool
    ↑                                                               ↓
    └────────── turn_boundary ←── post_tool_execution ←─────────────┘
```

## Run
```bash
# From the repository root
ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat \
  --example 011-hooks-guardrails --features jsonl-store
```
