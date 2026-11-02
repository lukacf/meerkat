---
name: schedule-workflow
description: How to author and inspect durable schedules from agent tools
requires_capabilities: [schedule]
---

# Schedule Workflow

Use Schedule when something should happen later, repeatedly, or on a wall-clock
calendar. Schedules own time and delivery. They do not own live work state.

## Operating Rules

- Create schedules with `meerkat_schedule_create` when the user asks for a
  reminder, recurrence, follow-up, monitor, wakeup, or routine automation.
- Choose the smallest trigger that matches the request: `once` for one future
  instant, `interval` for fixed cadence, `calendar` for wall-clock recurrence
  in a named timezone.
- For the current agent session, use the agent-facing session target
  `{"target_kind":"session","type":"current_session","action":{"type":"prompt","prompt":"Check in"}}`.
  The host resolves it to a persisted durable target.
- Use `resumable_session` only when you have the exact durable `session_id`,
  and `materialize_on_demand_session` when the schedule must create a session
  from an explicit build spec on first fire.
- The defaults are `misfire_policy: {"type":"skip"}`,
  `overlap_policy: "skip_if_running"`, and
  `missing_target_policy: "mark_misfired"`. Change them only when the user asks
  for bounded catch-up, concurrency, or different missing-target behavior.
- Inspect with `meerkat_schedule_get`, `meerkat_schedule_list`, and
  `meerkat_schedule_occurrences` before creating duplicates.
- `meerkat_schedule_list` returns one page of summaries (50 by default, at
  most 200 whatever `limit` asks; continue with `cursor` from
  `page.next_cursor`). A summary
  names the schedule, its trigger and target, and the first 160 characters of
  its action text (`action_head`; `action_chars` is the full length). Read the
  whole action, policies and options with `meerkat_schedule_get`.
- A long-running schedule has a long occurrence history, and
  `meerkat_schedule_occurrences` returns it one page of summaries at a time
  (50 by default): no trigger or target action, and only the first 160
  characters of a runtime outcome or failure detail. To check one occurrence
  whole, pass its `occurrence_id`; to check a window, pass `due_after_utc` /
  `due_before_utc` (inclusive); add `view: "full"` only when you need each
  occurrence's trigger and target. Match receipts to
  occurrences by `occurrence_id`, never by time of day.
- Pause for temporary suspension, resume to reactivate, update to change future
  behavior, and delete only when the schedule should stop permanently. Delete
  is irreversible, but historical occurrences remain queryable.

## Boundaries

- Use WorkGraph for pending work, dependencies, claims, and evidence.
- Use Schedule for time. A scheduled prompt may ask an agent to inspect
  WorkGraph, but Schedule should not duplicate WorkGraph readiness logic.
- Treat an occurrence claim and delivery outcome as Schedule authority only.
  They do not authorize a WorkGraph claim or prove that the underlying shared
  work completed.
- Do not infer successful delivery from a due time. Inspect occurrence phase;
  pending, claimed, dispatching, awaiting completion, completed, skipped,
  misfired, superseded, and delivery failed are distinct states.
- Use memory for recalled knowledge, not for future wakeups.
- Use builtin tasks for private scratch items that do not need scheduled
  delivery or shared durable coordination.
