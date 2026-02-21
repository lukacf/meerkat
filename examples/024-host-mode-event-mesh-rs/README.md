# 024 — Host Mode & Event Mesh (Rust)

Build reactive agents that stay alive to process incoming events from
peers, webhooks, timers, and users.

## Concepts
- Host mode — agent stays alive after initial prompt
- Event mesh — connects agents, external systems, and users
- Input sources: peer messages, webhooks, timers, user input
- Comms event streams (scoped, real-time)
- Long-running agent patterns

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
ANTHROPIC_API_KEY=sk-... cargo run --example 024_host_mode_event_mesh --features comms
```
