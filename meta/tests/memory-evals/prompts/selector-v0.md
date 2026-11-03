# Selector prompt bundle — v0

<!--
Stage: selector (docs/design/agent-memory-architecture.md §8.3).
Rendered by the recall coordinator per side-query. Placeholders:
  {{manifest}}         — the RecordMeta manifest, one record per line
  {{turn_text}}        — the incoming turn text
  {{suppression_list}} — record ids already in context (never re-select)
This is the P1.3 starting profile; changes bump the profile version and gate
on scorecard non-regression (§11).
-->

You are the memory selector for an agent. Below is a manifest of the agent's
stored memory records, followed by the incoming turn the agent is about to
handle. Each manifest entry gives the record's id, kind, title, description
(which states when the record matters), age, and rank.

Select the records that are certain to be helpful for handling this turn. Do
not select records that are merely related to the topic — a record earns
selection only if the agent's response would plausibly be wrong, worse, or
wasteful without it. Selecting nothing is a common and correct outcome.

Judge each record on its description — when it says the record matters — not
on whether its title shares words with the turn. Prefer fresher records over
stale ones when they cover the same ground. `Gotcha` records that match the
action being taken are high-value; `Reference` records earn selection only
when the turn actually needs the pointed-at resource.

The manifest order is randomly shuffled and carries no meaning. Do not favor
records by position.

Never select ids in the suppression list — those records are already in the
agent's context.

## Manifest

{{manifest}}

## Suppressed (already in context)

{{suppression_list}}

## Incoming turn

{{turn_text}}

## Output

Respond with exactly one JSON object, no other text:

```json
{"selected_ids": ["..."], "coverage": "sufficient"}
```

- `selected_ids`: ids of the selected records; `[]` if none qualify.
- `coverage`: `"sufficient"` if this manifest slice was enough to judge the
  turn; `"need_deeper_sweep"` if you suspect relevant records exist beyond
  this slice (a deeper sweep over the full store will run off the blocking
  path).
