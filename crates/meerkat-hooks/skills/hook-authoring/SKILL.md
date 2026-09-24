---
name: Hook Authoring
description: Writing hooks for the 14 hook points, execution modes, and typed decision semantics
requires_capabilities: [hooks]
---

# Hook Authoring

Use hooks for runtime observation and policy decisions at typed lifecycle
points. Hooks can observe and, at policy-capable points, allow or deny; they
should not become hidden owners of runtime truth.

## Hook Points

Meerkat provides 14 hook points across the agent, runtime, and comms lifecycles:

1. **RunStarted** - When the agent run begins
2. **PreLlmRequest** - Before sending to the LLM
3. **PostLlmResponse** - After receiving the LLM response
4. **PreToolExecution** - Before executing a tool call
5. **PostToolExecution** - After tool execution completes
6. **TurnBoundary** - At the boundary between turns
7. **RunCompleted** - When the agent run completes successfully
8. **RunFailed** - When the agent run fails
9. **RuntimeInputAccepted** - After runtime authority durably admits an input
10. **RuntimeInputRejected** - After an input is terminally rejected without an admission commit
11. **RuntimeInputDeduplicated** - When an idempotent submission resolves to an existing admitted input
12. **PeerIngressCommitted** - After a typed peer input is committed by runtime admission
13. **PeerEgressCommitted** - When a peer send reaches the successful outcome proved by its transport and local lifecycle authority
14. **InteractionCompleted** - After a correlated interaction completion is durably published

The last six points are observe-only: they report already-committed or
terminally-resolved facts. They require `capability: observe` in either
foreground or background mode. They cannot allow, deny, or otherwise change
the outcome being observed.

## Execution Modes

For the eight agent-loop points (`RunStarted` through `RunFailed` in the list):

- Foreground hooks run in ascending priority, then registration order. A deny
  short-circuits later foreground hooks at that point.
- Background hooks run concurrently and must declare `capability: observe`.
  Their decisions are discarded. Use them for logging and analytics, not
  policy enforcement.

The six observe-only points use post-commit dispatch, which does not await
hook execution or grant policy authority even when an entry is foreground.

For all points, runtime adapters are in-process handlers, commands, or HTTP
endpoints. Each entry has a typed adapter, timeout, point, mode, capability,
and priority.

## Decision Semantics

At policy-capable points, foreground hooks return one of:

- Allow: proceed normally.
- Deny: block the operation with a reason.
- Observe only: return no decision and no patches.

At the six observe-only points, return no decision and no patches. A denial
reported to the post-commit dispatcher, or a hook execution failure, is logged
without changing the committed or terminally-resolved fact.

## Boundaries

Semantic hook patches are retired. Hooks can observe typed projections and
deny at policy-capable points through the typed decision shape; provider
parameters, assistant text, tool arguments/results, and final run text remain
owned by the runtime/tool/LLM authority that produced them.

There is no per-hook `failure_policy`. Invalid configuration, execution
failure, and timeout are typed engine errors. At the eight agent-loop points,
foreground failures fail the run; background failures are recorded as dropped
background dispatches and do not become hook-local denials. Post-commit
observation failures cannot undo the outcome already observed.

Tool hook projections carry optional `ToolProvenance`, and LLM-response
projections carry typed provider-native `server_tool_content`. Classify those
typed fields synchronously instead of parsing display text or tool names.

Use WorkGraph for durable work state, Schedule for time, and memory for
knowledge retrieval. Hooks may observe those surfaces, but they do not own
their semantics.
