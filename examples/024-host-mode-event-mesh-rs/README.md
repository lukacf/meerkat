# 024 — Host Mode & Event Mesh (Rust)

Build reactive agents that stay alive to process incoming events from
peers, webhooks, timers, and users.

## What This Example Does

Creates an incident-response coordinator agent using `EphemeralSessionService`
(the same infrastructure that backs CLI, REST, RPC, and MCP Server). The agent
processes three turns that simulate real-time event injection:

1. **Turn 1**: Initial alert (CPU spike on prod-web-03)
2. **Turn 2**: Monitoring update (memory high, recent deploy found)
3. **Turn 3**: Resolution (rollback, metrics normalizing)

Each turn streams `AgentEvent`s and the agent maintains full conversation
context across all turns — demonstrating the reactive processing pattern
that host mode enables.

## Concepts
- `EphemeralSessionService` — in-memory session lifecycle with dedicated tokio tasks
- `create_session()` — creates a session and runs the initial turn
- `start_turn()` — injects a new prompt into a live session (the event injection primitive)
- `AgentEvent` streaming — real-time events across multiple turns
- Session state reads — observe accumulating context between turns
- `archive()` — clean shutdown of the session task

## When to Use Host Mode
| Scenario | Standard Mode | Host Mode |
|----------|--------------|-----------|
| Single query | Yes | No |
| Multi-turn chat | Yes | Better |
| Mob orchestrator | No | Yes |
| Event processor | No | Yes |
| Webhook handler | No | Yes |

## Run
```bash
ANTHROPIC_API_KEY=... cargo run --example 024-host-mode-event-mesh --features jsonl-store
```
