---
name: workgraph-workflow
description: How to use WorkGraph for durable commitments, dependencies, claims, and evidence
requires_capabilities: [work_graph]
---

# WorkGraph Workflow

Use WorkGraph when work must survive sessions, compaction, restarts, schedules,
or coordination between agents. It is the shared commitment graph, not private
scratch space and not semantic memory.

## Operating Rules

- Use `workgraph_create` for a new durable commitment. Keep related work in the
  same namespace; omitted namespace means `default`.
- Use `workgraph_ready` to find eligible work. Do not infer readiness from item
  fields, blocker counts, due times, or edges yourself.
- Claim an item before doing durable or shared work with `workgraph_claim`.
  Include your typed owner and the current `expected_revision`. Choose either
  `lease_seconds` or `lease_expires_at`, never both.
- If a write fails with a stale revision, reload the item with `workgraph_get`
  or `workgraph_snapshot`, reconsider the current state, then retry only if the
  work still makes sense.
- Every successful mutation advances the item revision. Use the returned item,
  not an older cached revision, for the next claim, update, evidence, release,
  block, or close operation.
- Use `workgraph_link` for real dependencies and relationships. Edges are
  permanent; there is no unlink. Use `blocks` (`from_id` blocker, `to_id`
  blocked) only when the target must not be ready until the source is
  `completed`; a `failed` or `cancelled` blocker keeps the target blocked.
- Use `parent` when an item is one part of a larger commitment. The edge
  points from the child (`from_id`) to the parent (`to_id`); add it right
  after creating the child, because `workgraph_create` has no parent field.
  A parent is not ready while any child is live, and a `failed` or
  `cancelled` child keeps a `require_success` parent waiting. The parent's
  `failed_child_join_policy` or `cancelled_child_join_policy` changes that:
  `accept` lets the parent proceed without that child, while `propagate`
  closes the parent with the child's status.
- Attach evidence with `workgraph_add_evidence` for artifacts, PRs, logs,
  summaries, external tickets, or other proof that the work changed state.
- Close with `workgraph_close` only when terminal truth exists, and always pass
  `status`. `completed` means the titled outcome happened. `failed` means it
  was attempted and did not happen: a refuted hypothesis or a fix that did not
  work is `failed`, with the refutation attached as evidence. `cancelled`
  means the work was dropped without a verdict (superseded, out of scope).
  Omitting `status` records `completed`, which misfiles anything that did not
  succeed. A non-self-attested completion policy can require typed
  confirmation evidence before close.
- Release a claim with `workgraph_release` when you are stopping before terminal
  completion and the work should be claimable by someone else.
- `workgraph_list` and `workgraph_snapshot` omit terminal items (`completed`,
  `cancelled`, `failed`) unless `include_terminal` is true, even when
  `statuses` names a terminal status. `workgraph_ready` returns live eligible
  items only. To review closed work, pass `include_terminal: true` or read
  `workgraph_events`.
- A `labels` filter on list, ready, or snapshot matches only items that carry
  every listed label. Filter on one label to widen the result.
- Use `workgraph_events` for audit history and `workgraph_snapshot` for a
  graph-wide view. Do not reconstruct either from peer chat or task lists.
- `workgraph_policy_escalate` and `workgraph_attention_reassign` are available
  only with a runtime-bound attention authority. Their absence on an unscoped
  surface is intentional.

## Boundaries

- Use builtin `task_*` tools for private, local, lightweight scratch tracking.
- Use WorkGraph for shared durable commitments, dependencies, readiness, claims,
  and evidence.
- Use Schedule for time-based wakeups and recurrence. A schedule can wake an
  agent, but WorkGraph remains the live work state.
- Use memory for knowledge retrieval and historical context. Memory does not
  own live work state.

## Typical Loop

1. Call `workgraph_ready` for the active realm and namespace.
2. Pick an item that matches the current objective.
3. Claim it with the current item revision as `expected_revision`.
4. Do the work.
5. Add evidence for durable outputs.
6. Update the item if the scope, timing, priority, or labels changed.
7. Close it with an explicit `status` only when the outcome is terminal, or
   release it if another agent should continue.
