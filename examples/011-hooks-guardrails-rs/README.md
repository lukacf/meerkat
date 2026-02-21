# 011 — Hooks & Guardrails (Rust)

Intercept and control agent behavior at 7 defined hook points. Use hooks for
audit logging, content filtering, approval gates, cost tracking, and more.

## Concepts
- `HookPoint` — 7 interception points in the agent loop
- `HookCapability` — observe (read-only) or gate (Allow/Deny/Rewrite)
- `HookExecutionMode` — foreground (blocking) or background (async)
- `HookRuntimeConfig` — command, HTTP, or in-process execution
- `HookFailurePolicy` — what happens when a hook fails
- `DefaultHookEngine` — the standard hook processor

## Hook Points
```
pre_llm_call → LLM → post_llm_response → pre_tool_dispatch → Tool
    ↑                                                           ↓
    └──────── turn_boundary ←── post_tool_result ←──────────────┘
                    ↓
         pre_compaction → Compactor → post_compaction
```

## Run
```bash
ANTHROPIC_API_KEY=sk-... cargo run --example 011_hooks_guardrails
```
