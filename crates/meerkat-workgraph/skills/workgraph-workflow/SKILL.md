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
  same host-granted namespace. Omitting `namespace` uses the namespace bound
  by the host's grant; it is `default` only when that is the granted namespace.
  The grant authorizes one immutable realm/namespace scope. Supplying another
  namespace string does not switch scope — it is rejected.
- Use `workgraph_ready` to find eligible work. Do not infer readiness from item
  fields, blocker counts, due times, or edges yourself.
- Claim an item before doing durable or shared work with `workgraph_claim`.
  Include your typed owner and the current `expected_revision`. Set
  `lease_seconds` for a lease, or omit it for no expiry. The tool computes
  expiry from the store's claim observation time.
- Claim works only on an `open` item, or on an `in_progress` item whose lease
  has expired. A `blocks` edge keeps the dependent item `open`: it is not ready
  while that blocker is unresolved. A blocker completing removes only that
  gate; the item is claimable only when every other readiness condition also
  holds (no other unresolved blocker, its child join satisfied, no future
  `not_before`, not snoozed). `workgraph_block`
  is different: it sets the item's status to `blocked`, which cannot be claimed,
  and there is currently no transition back to `open` (an update keeps it
  `blocked`). Use it only for work that should stay parked until it is closed.
- Closing is not a way to unblock. `completed` needs the titled outcome to have
  actually happened. Cancelling, or replacing the work with a new item, must be a
  real decision: keep the history (attach evidence, link the replacement) and do
  not create a duplicate of work that already exists.
- If a write fails with a stale revision (the `expected_revision` you passed
  is no longer the item's), reload the item with `workgraph_get`
  or `workgraph_snapshot`, reconsider the current state, then retry only if the
  work still makes sense.
- Every successful mutation advances the item revision. Use the returned item,
  not an older cached revision, for the next claim, update, evidence, release,
  block, or close operation.
- The two rules above are for ordinary tool use. A turn that carries a
  `WorkGraph attention projection` block runs under an attention binding, and
  there they do not hold. Every WorkGraph call of such a turn, a read included,
  is served only while that projection is still current, and the projection is
  fixed for the turn. Your own first write to the bound item changes the item,
  so every later WorkGraph call of the same turn is refused with "stale or
  inactive WorkGraph attention projection", `workgraph_get` included. That is
  not a revision conflict: reloading, or passing another `expected_revision`,
  does not help. In such a turn read first if you need to, make the one write
  that matters most, stop calling WorkGraph tools after it, and say in your
  reply what is left to do. A later turn carries a current projection.
  Nothing starts that turn for you: it starts when another input reaches you.
  The same refusal on a turn's first call means the item, its binding or its
  parent changed after the turn started.
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
- To withdraw evidence (a wrong reading, a superseded result), call
  `workgraph_add_evidence` with a new ref that lists the earlier ref's `id`
  in `withdraws`. Nothing is removed: a ref whose `id` a later ref lists in
  `withdraws` is withdrawn and stays on the item. Withdraw only earlier refs
  on the same item, never the new ref's own `id` or another withdrawal;
  confirmation and execution evidence cannot be withdrawn.
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
- `workgraph_list`, `workgraph_ready`, and `workgraph_snapshot` return one
  page: at most `limit` items (100 by default), list and snapshot most
  recently updated first, ready by priority and then age. A result with
  `truncated: true` has more matching items (`total_items` counts them all);
  pass its `next_cursor` back as `cursor` to read the next page rather than
  raising `limit`. `workgraph_events` pages forward with `after_seq` set to
  the previous page's `next_after_seq`.
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

1. Call `workgraph_ready` within the host-granted realm and namespace.
2. Pick an item that matches the current objective.
3. Claim it with the current item revision as `expected_revision`.
4. Do the work.
5. Add evidence for durable outputs.
6. Update the item if the scope, timing, priority, or labels changed.
7. Close it with an explicit `status` only when the outcome is terminal, or
   release it if another agent should continue.

Steps 5 to 7 in one turn are for ordinary tool use. In a turn under an
attention binding only one of them can be done on the bound item; the others
wait for later turns (see Operating Rules).
