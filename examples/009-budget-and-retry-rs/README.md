# 009 — Budget & Retry Policies (Rust)

Production guardrails: token budgets, tool-call limits, and retry policies for
resilient agent execution.

## Concepts
- `BudgetLimits` — measured token exhaustion thresholds, tool-call limits, and time budgets
- `RetryPolicy` - exponential backoff for provider failures classified as
  retryable
- Applying a retry policy to an `AgentBuilder`
- Handling budget exhaustion returned by the agent run

## Budget Types
| Limit | Description |
|-------|-------------|
| `max_tokens` | Cumulative measured token threshold for exhaustion/continuation |
| `max_tool_calls` | Max tool invocations |
| `max_duration` | Agent-lifetime wall-clock budget; not reset for each run |
| `max_turn_duration` | Optional aggregate per-run time budget, re-armed on each run |
| `max_tokens_per_turn` | Per-LLM-request output-token limit on `AgentBuilder` (512 in this example), not a cumulative billing cap |

Usage is charged when the provider reports measured accounting. An in-flight
call can exceed `max_tokens`; the remaining cumulative budget is not a hard
limit on the next request's tokens. Missing accounting does not advance the
token counter. Do not use this threshold as a guaranteed billing ceiling.

Token exhaustion can return `Ok(RunResult)` with the typed
`terminal_cause_kind = BudgetExhausted`. The example reports that separately
from normal completion, handles only typed time-budget errors as expected, and
propagates unrelated failures. The response preview counts Unicode scalar
values, not bytes, and adds an ellipsis only when truncated.

Both JSONL stores are guarded scratch directories under the current directory
and are removed on normal completion or returned errors (not forced termination).

## Retry Strategy

For typed retryable provider failures, the configured policy has this nominal
exponential backoff, before jitter and delay selection. The example prints
the policy but does not force a live provider failure.

```
Attempt 1 → fail → wait 500ms →
Attempt 2 → fail → wait 1s →
Attempt 3 → fail → wait 2s →
Attempt 4 → give up
```

The computed backoff includes ±10% jitter. Actual waits may instead be
selected from provider retry hints or the rate-limit policy, and can be
shortened by the remaining duration budget.

## Run
```bash
# From the repository root
ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat \
  --example 009-budget-and-retry --features jsonl-store
```
