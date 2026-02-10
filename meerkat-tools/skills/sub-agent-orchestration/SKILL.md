---
name: Sub-Agent Orchestration
description: When to agent_spawn vs agent_fork, model selection, result aggregation
requires_capabilities: [sub_agents]
---

# Sub-Agent Orchestration

## Spawn vs Fork

- `agent_spawn`: Creates a new agent with its own context. Use for independent tasks.
- `agent_fork`: Creates a child agent sharing parent context. Use for parallel subtasks.

## Model Selection

Choose the right model for sub-agents:
- Fast models for simple queries and data extraction
- Capable models for complex reasoning and code generation
- Match the model to the task complexity

## Result Aggregation

- Use `agent_status` to check completion
- Use `agent_list` to see all active agents
- Collect results from completed agents

## Concurrency Limits

- Sub-agents share the parent's concurrency limits
- Don't spawn more agents than the limit allows
- Cancel unnecessary agents to free slots
