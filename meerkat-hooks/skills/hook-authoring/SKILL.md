---
name: Hook Authoring
description: Writing hooks for the 8 hook points, execution modes, and decision semantics
requires_capabilities: [hooks]
---

# Hook Authoring

## Hook Points

Meerkat provides 8 hook points in the agent lifecycle:
1. **RunStarted** — When the agent run begins
2. **PreLlmRequest** — Before sending to the LLM
3. **PostLlmResponse** — After receiving LLM response
4. **PreToolExecution** — Before executing a tool call
5. **PostToolExecution** — After tool execution completes
6. **TurnBoundary** — At the boundary between turns
7. **RunCompleted** — When the agent run completes successfully
8. **RunFailed** — When the agent run fails

## Execution Modes

- **Foreground**: Blocks execution and can deny. Use for policy enforcement.
- **Background**: Runs concurrently, fire-and-forget. Use for logging, analytics.

## Decision Semantics

Hooks return one of:
- **Allow**: Proceed normally
- **Deny**: Block the operation with a reason
- **Observe only**: Return no decision and no patches

## Patch Format

Semantic hook patches are retired. Hooks can observe typed projections and
deny through the typed decision shape; provider parameters, assistant text,
tool arguments/results, and final run text remain owned by the runtime/tool/LLM
authority that produced them.

## Failure Policy

`failure_policy` is retained for compatibility. Current hook runtime failures
fail closed through typed engine errors; they are not converted into
warning-only success or hook-local denials by `fail_open` / `fail_closed`.
