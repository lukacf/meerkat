# 005 — Streaming Events (Rust)

Real-time event processing from agent execution. Every LLM delta, tool call,
and state transition is surfaced as a typed `AgentEvent`.

## Concepts
- `AgentEvent` — the full event taxonomy (text deltas, tool calls, turns, errors)
- `run_with_events()` — agent execution with an event channel
- `spawn_event_logger()` — built-in helper for CLI-like streaming
- Custom event processing with `mpsc::Receiver`

## Event Types
| Event | Description |
|-------|-------------|
| `TextDelta` | Incremental text from the LLM |
| `ToolCallRequested` | Agent wants to invoke a tool |
| `ToolResultReceived` | Tool execution completed |
| `TurnCompleted` | One agent loop iteration finished |
| `BudgetUpdate` | Token/time budget consumption |

## Run
```bash
ANTHROPIC_API_KEY=sk-... cargo run --example 005_streaming_events
```
