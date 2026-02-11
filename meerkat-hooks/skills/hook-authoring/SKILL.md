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

- **Foreground**: Blocks execution, can modify/deny. Use for policy enforcement.
- **Background**: Runs concurrently, fire-and-forget. Use for logging, analytics.
- **Observe**: Receives events but cannot block. Use for monitoring.

## Decision Semantics

Hooks return one of:
- **Allow**: Proceed normally
- **Deny**: Block the operation with a reason
- **Rewrite**: Modify the operation (e.g., rewrite tool arguments)

## Patch Format

For `PreToolExecution`, hooks can rewrite tool arguments using JSON patches.

## Failure Policy

- **fail-closed**: Hook failure blocks the operation (safer)
- **fail-open**: Hook failure allows the operation (more resilient)
