---
name: Task Workflow
description: How to use builtin task tools for private lightweight work tracking
requires_capabilities: [builtins]
---

# Task Workflow

Use builtin task tools for lightweight project work tracking. A configured
store may persist them, but they remain scratch structure rather than the
claim-safe shared WorkGraph authority.

## Operating Rules

- Use `task_create` for planning, checklists, or local project tasks that do
  not need realm-wide claims, evidence, or cross-agent readiness.
- Use clear imperative subjects and descriptions with acceptance criteria.
- Read an exact task with `task_get`, and use `task_update` to move it to
  `in_progress` when you start and `completed` only when it is actually done.
- Use `task_list` after completing a task to find the next local item.
- Keep dependencies simple. If blocking relationships must coordinate multiple
  agents or survive compaction/restarts as shared truth, use WorkGraph instead.

## Boundary With WorkGraph

- Builtin tasks: lightweight project task lists and simple progress tracking;
  no claims, leases, revisions, or evidence authority.
- WorkGraph: realm-scoped durable commitments, readiness, dependency topology,
  authorized claims, leases, evidence, and terminal truth. Its ledger is the
  shared authority, not a peer message, prompt, actor, or task-list projection.
- Schedule: time-based wakeups, recurrence, occurrence claims, and delivery
  outcomes. It does not decide WorkGraph readiness or completion.
- Memory: recalled knowledge, not live task state.
