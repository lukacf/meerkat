# 011 — Hooks & Guardrails (Rust)

Intercept and control agent behavior at eight agent-loop hook points. Use hooks for
audit logging, content filtering, approval gates, cost tracking, and more.

## Concepts

- `HookPoint` - eight agent-loop points plus six runtime-only observation points
- `HookCapability` - observe (read-only) or guardrail (Allow/Deny)
- `HookExecutionMode` - foreground (blocking) or background (async)
- `HookAdapterConfig` - command, HTTP, or in-process execution
- `DefaultHookEngine` - the standard hook processor

## Agent-loop Hook Points
1. `run_started`
2. `pre_llm_request`
3. `post_llm_response`
4. `pre_tool_execution`
5. `post_tool_execution`
6. `turn_boundary`
7. `run_completed`
8. `run_failed`

The additional runtime observation points are `runtime_input_accepted`,
`runtime_input_rejected`, `runtime_input_deduplicated`, `peer_ingress_committed`,
`peer_egress_committed`, and `interaction_completed`; they are not exercised by
this standalone agent-loop example. Supported capabilities are `observe` and
`guardrail`, not `rewrite`.

## Command observer

The printed configuration's cost tracker runs the included `cost_tracker.py`
with Python 3 from the repository root. It consumes one JSON invocation on stdin,
records session/turn identity and usage from `post_llm_response`, and emits `{}` as
the valid hook response on stdout. It does not log prompt/response text or read an
imaginary payload environment variable. Input is bounded at 64 KiB and each log
record at 8 KiB; failures are reported through the hook engine.

The example log destination is `.rkat/example-hooks/costs.jsonl`; change that
argument to a path you own before enabling the observer. Logs append and are
retained until you remove them; rotate or delete this example-owned log as needed.
Other printed HTTP/command registrations are templates requiring your own
endpoint or script. The runnable agent itself uses in-process observers.

## Run
```bash
# From the repository root
ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat \
  --example 011-hooks-guardrails --features jsonl-store
```
