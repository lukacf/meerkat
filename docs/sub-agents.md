# Sub-Agents

Meerkat supports hierarchical agent workflows where an LLM agent can spawn and manage independent sub-agents. Sub-agents run concurrently in background tokio tasks and can use different providers/models (e.g., a Claude parent spawning Gemini or GPT sub-agents).

## Feature Flag

Sub-agents require the `sub-agents` feature flag, which is enabled by default in both the CLI and the facade crate.

```toml
# In Cargo.toml
meerkat = { features = ["sub-agents"] }
```

At runtime, the `AgentFactory` has `enable_subagents` (default: `false`). Per-request builds can override via `AgentBuildConfig::override_subagents`.

```rust
let factory = AgentFactory::new(store_path)
    .builtins(true)
    .subagents(true);
```

## Available Tools

Sub-agents provide five tools, all disabled by default and gated behind the `sub-agents` feature and factory configuration:

| Tool | Description |
|---|---|
| `agent_spawn` | Create a sub-agent with clean context (just the prompt) |
| `agent_fork` | Create a sub-agent that inherits the full conversation history |
| `agent_status` | Get the status and output of a sub-agent by ID |
| `agent_cancel` | Cancel a running sub-agent |
| `agent_list` | List all sub-agents with their current states |

## Spawn vs Fork

### `agent_spawn`

Creates a new sub-agent with a **clean context**. The sub-agent starts fresh with only the provided prompt -- it does not inherit conversation history.

**Use for**: Independent tasks that do not need the parent's context.

**Parameters**:

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `prompt` | `string` | Yes | -- | Initial prompt/task for the sub-agent. |
| `provider` | `string` | No | `"anthropic"` | LLM provider: `anthropic`, `openai`, `gemini`. |
| `model` | `string` | No | Provider default | Model name. Must be in the allowlist for the provider. |
| `tool_access` | `object` | No | `inherit` | Tool access policy (see below). |
| `budget` | `object` | No | 50,000 tokens | Budget limits: `{ max_tokens, max_turns, max_tool_calls }`. |
| `system_prompt` | `string` | No | Inherited or none | Override the system prompt for this sub-agent. |
| `host_mode` | `boolean` | No | `false` | Keep agent alive processing comms messages after initial prompt. Requires `comms` feature. |

**Response**:

```json
{
  "agent_id": "019467d9-7e3a-7000-8000-000000000000",
  "name": "sub-agent-019467d97e3a",
  "provider": "anthropic",
  "model": "claude-sonnet-4-5",
  "state": "running",
  "message": "Sub-agent spawned successfully. Use agent_status to check progress."
}
```

### `agent_fork`

Creates a sub-agent that **inherits the full conversation history** from the parent. The fork prompt is appended as a new user message to the cloned session.

**Use for**: Parallel exploration of alternatives, or delegating subtasks that need the parent's context.

**Parameters**:

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `prompt` | `string` | Yes | -- | Additional prompt appended to the inherited conversation history. |
| `provider` | `string` | No | `"anthropic"` | LLM provider. |
| `model` | `string` | No | Provider default | Model name. |
| `tool_access` | `object` | No | `inherit` | Tool access policy. |
| `budget_policy` | `string` | No | `"equal"` | Budget allocation: `equal`, `remaining`, `proportional`, or `fixed:<tokens>`. |

**Response** includes `messages_inherited` count showing how many messages were copied from the parent.

**Note**: Forked agents do not run in host mode and inherit their system prompt from the session.

## Tool Access Policies

Both `agent_spawn` and `agent_fork` accept a `tool_access` parameter that controls which tools the sub-agent can use:

| Policy | Description |
|---|---|
| `{"policy": "inherit"}` | Inherit all tools from the parent (default). |
| `{"policy": "allow_list", "tools": ["shell", "task_list"]}` | Only allow the specified tools. |
| `{"policy": "deny_list", "tools": ["shell"]}` | Block the specified tools, allow everything else. |

The `tool_access` value can be provided as either a JSON object or a JSON-encoded string (for LLMs that stringify nested objects).

**Important**: Sub-agents do NOT receive sub-agent tools (`agent_spawn`, `agent_fork`, etc.) in their tool set. This is enforced by the factory, which clones itself with `.subagents(false)` when building the sub-agent's tool dispatcher. This prevents uncontrolled recursive spawning (though nested spawning can be enabled via config -- see Concurrency Limits).

## Monitoring Sub-Agents

### `agent_status`

Query a single sub-agent by its UUID.

**Parameters**: `{ "agent_id": "<UUID>" }`

**Response**:

```json
{
  "agent_id": "019467d9-7e3a-7000-8000-000000000000",
  "state": "completed",
  "output": "The analysis shows...",
  "is_final": true,
  "duration_ms": 12500,
  "tokens_used": 3200
}
```

States: `running`, `completed`, `failed`, `cancelled`.

When `is_final: true`, the sub-agent is done. The `output` field contains the result text (or `error` field for failures).

### `agent_list`

List all sub-agents with summary counts.

**Parameters**: None (empty object).

**Response**:

```json
{
  "agents": [
    {
      "id": "019467d9-7e3a-7000-8000-000000000000",
      "name": "sub-agent-019467d97e3a",
      "state": "running",
      "depth": 1,
      "running_ms": 5000
    }
  ],
  "running_count": 1,
  "completed_count": 0,
  "failed_count": 0,
  "total_count": 1
}
```

### `agent_cancel`

Cancel a running sub-agent. Only agents in the `running` state can be cancelled.

**Parameters**: `{ "agent_id": "<UUID>" }`

**Response**: `{ "success": true, "previous_state": "running", "message": "Sub-agent cancelled successfully" }`

## Concurrency Limits

Defined in `meerkat-core/src/ops.rs` as `ConcurrencyLimits`:

| Field | Type | Default | Description |
|---|---|---|---|
| `max_depth` | `u32` | `3` | Maximum nesting depth for sub-agents. |
| `max_concurrent_ops` | `usize` | `32` | Maximum concurrent operations (across all types). |
| `max_concurrent_agents` | `usize` | `8` | Maximum concurrently running sub-agents. |
| `max_children_per_agent` | `usize` | `5` | Maximum children a single agent can spawn. |

The `SubAgentManager` enforces these limits. Attempting to spawn beyond the limits returns an error.

## SubAgentConfig

Defined in `meerkat-tools/src/builtin/sub_agent/config.rs`:

| Field | Type | Default | Description |
|---|---|---|---|
| `default_provider` | `String` | `"anthropic"` | Default provider when not specified in spawn/fork calls. |
| `default_model` | `Option<String>` | `None` | Default model. If `None`, uses the first model in the provider's allowlist. |
| `concurrency_limits` | `ConcurrencyLimits` | See above | Limits for this agent's sub-agents. |
| `allow_nested_spawn` | `bool` | `true` | Whether sub-agents can spawn further sub-agents. |
| `max_budget_per_agent` | `Option<u64>` | `None` | Cap on tokens for any single sub-agent. Requested budgets are clamped to this value. |
| `default_budget` | `Option<u64>` | `Some(50_000)` | Default token budget when not specified. |
| `inherit_system_prompt` | `bool` | `true` | Whether spawned agents inherit the parent's tool usage instructions as their system prompt. |
| `enable_comms` | `bool` | `false` | Whether to enable parent-child communication via comms. |

## Budget Allocation

### Spawn Budget

For `agent_spawn`, the budget is resolved as:
1. If `budget.max_tokens` is specified, use it (capped by `max_budget_per_agent` if set).
2. Otherwise, use `default_budget` (50,000 tokens by default).

### Fork Budget Policies

For `agent_fork`, the `budget_policy` parameter controls allocation:

| Policy | Behavior |
|---|---|
| `equal` (default) | Each fork gets `default_budget` tokens. |
| `remaining` | Fork gets the parent's remaining budget (currently falls back to `equal`). |
| `proportional` | Proportional allocation (currently falls back to `equal`). |
| `fixed:<N>` | Fixed budget of N tokens (capped by `max_budget_per_agent`). |

## Model Allowlists

Sub-agents validate requested models against a per-provider allowlist. The allowlist comes from either:

1. **Resolved policy** from the global `SubAgentsConfig` in the configuration file (highest precedence).
2. **Default allowlist** from `SubAgentsConfig::default()` (hardcoded fallback).

The `agent_spawn` tool definition dynamically includes the allowed models in the `model` parameter description, so the LLM can see which models are available.

## Sub-Agent Execution Lifecycle

1. **Validation**: The tool validates the prompt (non-empty), provider, model (against allowlist), and concurrency limits.
2. **Client creation**: An `LlmClient` is created via `LlmClientFactory` for the requested provider.
3. **Session setup**:
   - `agent_spawn`: Creates a new session with optional system prompt + user prompt.
   - `agent_fork`: Clones the parent session and appends the fork prompt.
4. **Registration**: The sub-agent is registered with the `SubAgentManager` before the task is spawned.
5. **Background execution**: A tokio task runs the agent loop (`agent.run_pending()` or `agent.run_host_mode()` for host mode).
6. **Completion**: On success, `manager.complete()` stores the result. On failure, `manager.fail()` stores the error. Both notify listeners via a `watch` channel.
7. **Result collection**: Completed results are stored in a bounded deque (`MAX_COMPLETED_AGENTS = 256`) and can be queried via `agent_status` or `agent_list`.

## Comms Integration

When the `comms` feature is enabled and the parent has comms configured:

1. Sub-agents get a comms context injected into their prompt explaining how to communicate with the parent.
2. The sub-agent's tools are wrapped with comms tools via `wrap_with_comms`.
3. The sub-agent is added to the parent's trusted peers so bidirectional communication works.
4. The parent receives `comms` instructions in the spawn response explaining how to message the child.
5. Communication uses in-process (`CommsBootstrap::for_child_inproc`) rather than network listeners.

## Factory Wiring

In `meerkat/src/factory.rs`, when `enable_subagents` is true:

1. The factory creates a `CompositeDispatcher` for builtin tools.
2. A second `CompositeDispatcher` is built for sub-agent tools (with `.subagents(false)` and `.comms(false)` to prevent recursive sub-agent tools).
3. A `SubAgentManager` is created with default `ConcurrencyLimits`.
4. A `SubAgentToolState` bundles the manager, client factory, tool dispatcher, session store, parent session, and config.
5. A `SubAgentToolSet` wraps all five tools with the shared state.
6. The tool set is registered on the parent's `CompositeDispatcher` via `register_sub_agent_tools()`.

## Best Practices

### When to Use Sub-Agents
- Parallel independent tasks (e.g., analyze multiple files simultaneously).
- Tasks requiring different model strengths (e.g., GPT for coding, Gemini for analysis).
- Long-running work you want to delegate while doing other things.
- Breaking complex problems into specialized subtasks.

### When NOT to Use Sub-Agents
- Simple tasks you can do directly.
- Tasks requiring tight coordination (use sequential steps instead).
- When you need immediate results (sub-agents add latency).

### Monitoring Tips
- Do NOT poll `agent_status` repeatedly -- this wastes tokens. The agent will notify when it is done.
- Use `agent_list` to see all sub-agents at once (more efficient than individual status checks).
- Do other useful work, then check status once or twice near the end.

## Architecture Summary

```
meerkat-core/src/sub_agent.rs                         -- SubAgentManager, SubAgentInfo, ConcurrencyLimits enforcement
meerkat-core/src/ops.rs                                -- ConcurrencyLimits, SpawnSpec, ToolAccessPolicy, ForkBudgetPolicy
meerkat-tools/src/builtin/sub_agent/mod.rs             -- Module root, re-exports
meerkat-tools/src/builtin/sub_agent/spawn.rs           -- AgentSpawnTool (agent_spawn)
meerkat-tools/src/builtin/sub_agent/fork.rs            -- AgentForkTool (agent_fork)
meerkat-tools/src/builtin/sub_agent/status.rs          -- AgentStatusTool (agent_status)
meerkat-tools/src/builtin/sub_agent/cancel.rs          -- AgentCancelTool (agent_cancel)
meerkat-tools/src/builtin/sub_agent/list.rs            -- AgentListTool (agent_list)
meerkat-tools/src/builtin/sub_agent/runner.rs          -- spawn_sub_agent(), spawn_sub_agent_dyn(), session creation
meerkat-tools/src/builtin/sub_agent/state.rs           -- SubAgentToolState (shared state for all tools)
meerkat-tools/src/builtin/sub_agent/config.rs          -- SubAgentConfig, SubAgentError
meerkat-tools/src/builtin/sub_agent/tool_set.rs        -- SubAgentToolSet (bundles all five tools)
meerkat/src/factory.rs                                 -- Factory wiring (build_builtin_dispatcher)
```
