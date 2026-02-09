---
name: Hook Authoring
description: Writing hooks for the 7 hook points, execution modes, and decision semantics
requires_capabilities: [hooks]
---

# Hook Authoring

## Hook Points

Meerkat provides 7 hook points in the agent lifecycle:
1. **PreLlmRequest** — Before sending to the LLM
2. **PostLlmResponse** — After receiving LLM response
3. **PreToolExecution** — Before executing a tool call
4. **PostToolExecution** — After tool execution completes
5. **PreTurnStart** — Before a new turn begins
6. **PostTurnEnd** — After a turn completes
7. **PreSessionCreate** — Before session creation

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
