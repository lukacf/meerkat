# 016 — Sub-Agent Orchestration (Rust)

Delegate subtasks to independent child agents. The parent orchestrates,
children execute, results flow back for synthesis.

## Concepts
- `spawn_agent` — built-in tool for creating child agents
- Budget isolation — children don't drain the parent's budget
- Spawn vs Fork — independent context vs shared history
- `SubAgentManager` — lifecycle control for child agents

## Patterns
| Pattern | Use Case |
|---------|----------|
| Spawn | Independent task with fresh context |
| Fork | Share parent's conversation history |
| Fan-out | Multiple children for parallel research |
| Pipeline | Chain children sequentially |

## Run
```bash
ANTHROPIC_API_KEY=sk-... cargo run --example 016_sub_agent_orchestration
```
