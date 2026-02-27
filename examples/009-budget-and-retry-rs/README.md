# 009 — Budget & Retry Policies (Rust)

Production guardrails: token budgets, time caps, and retry policies for
resilient agent execution.

## Concepts
- `BudgetLimits` — hard caps on tokens, tool calls, and duration
- `RetryPolicy` — exponential backoff for transient LLM failures (429, 500)
- `BudgetPool` — share a budget across multiple agents (not shown, see 016)
- Error handling for `AgentError::TokenBudgetExceeded` / `ToolCallBudgetExceeded`

## Budget Types
| Limit | Description |
|-------|-------------|
| `max_tokens` | Hard cap on cumulative token usage |
| `max_tool_calls` | Max tool invocations |
| `max_duration` | Wall clock timeout |

## Retry Strategy
```
Attempt 1 → fail → wait 500ms →
Attempt 2 → fail → wait 1s →
Attempt 3 → fail → wait 2s →
Attempt 4 → give up
```

## Run
```bash
# This is a reference implementation. For runnable examples, see meerkat/examples/.
```
