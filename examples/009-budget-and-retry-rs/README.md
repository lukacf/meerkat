# 009 — Budget & Retry Policies (Rust)

Production guardrails: token budgets, turn limits, time caps, and retry
policies for resilient agent execution.

## Concepts
- `BudgetLimits` — hard caps on tokens, turns, tool calls, and wall time
- `RetryPolicy` — exponential backoff for transient LLM failures (429, 500)
- `BudgetPool` — share a budget across multiple agents (not shown, see 016)
- Error handling for `AgentError::BudgetExhausted`

## Budget Types
| Limit | Description |
|-------|-------------|
| `max_total_tokens` | Hard cap on cumulative token usage |
| `max_turns` | Max agent loop iterations |
| `max_tool_calls` | Max tool invocations |
| `max_wall_time` | Wall clock timeout |

## Retry Strategy
```
Attempt 1 → fail → wait 500ms →
Attempt 2 → fail → wait 1s →
Attempt 3 → fail → wait 2s →
Attempt 4 → give up
```

## Run
```bash
ANTHROPIC_API_KEY=sk-... cargo run --example 009_budget_and_retry
```
