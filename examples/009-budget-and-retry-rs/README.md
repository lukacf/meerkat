# 009 — Budget & Retry Policies (Rust)

Production guardrails: token budgets, tool-call limits, and retry policies for
resilient agent execution.

## Concepts
- `BudgetLimits` — cumulative token-usage stopping limits, plus caps on tool calls and duration
- `RetryPolicy` - exponential backoff for provider failures classified as
  retryable
- Applying a retry policy to an `AgentBuilder`
- Handling budget exhaustion returned by the agent run

## Budget Types
| Limit | Description |
|-------|-------------|
| `max_tokens` | Cumulative accounted-token stopping limit; the final provider call may overshoot |
| `max_tool_calls` | Max tool invocations |
| `max_duration` | Wall clock timeout |

The cumulative token limit is checked around provider calls using accounted
usage, so a completed call can exceed the remaining budget before further work
is stopped. Separately, `max_tokens_per_turn` bounds the requested response
output (512 in this example), not cumulative input and output usage.

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
