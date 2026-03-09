# WASM Runtime API Surface

## wasm_bindgen Exports

### Bootstrap
| Export | Params | Returns | Notes |
|--------|--------|---------|-------|
| `init_runtime` | mobpack bytes, credentials JSON | init result JSON | Primary: mobpack-first bootstrap |
| `init_runtime_from_config` | config JSON | init result JSON | Advanced: bare-bones bootstrap |
| `runtime_version` | — | version string | Returns CARGO_PKG_VERSION for JS/WASM version validation |
| `register_tool_callback` | name, description, schema JSON, callback | `()` | Must be called BEFORE init |
| `clear_tool_callbacks` | — | `()` | Clear all registered JS tool callbacks |

### Session Lifecycle
| Export | Params | Returns | Notes |
|--------|--------|---------|-------|
| `create_session` | mobpack bytes, config JSON | handle (u32) | Direct AgentFactory path |
| `create_session_simple` | config JSON | handle (u32) | No mobpack, uses registered tool callbacks |
| `start_turn` | handle, prompt, options JSON | RunResult JSON | async, LLM call |
| `get_session_state` | handle | JSON | Session metadata |
| `destroy_session` | handle | `()` | Remove session |
| `poll_events` | handle | AgentEvent[] JSON | Drain events |

### Mob Lifecycle (delegates to MobMcpState)
| Export | Params | Returns | Notes |
|--------|--------|---------|-------|
| `mob_create` | definition JSON | mob_id string | async |
| `mob_spawn` | mob_id, specs JSON | result JSON | async, batch spawn |
| `mob_wire` | mob_id, a, b | `()` | async |
| `mob_unwire` | mob_id, a, b | `()` | async |
| `mob_retire` | mob_id, meerkat_id | `()` | async |
| `mob_list_members` | mob_id | RosterEntry[] JSON | |
| `mob_send_message` | mob_id, meerkat_id, msg | `()` | async |
| `mob_respawn` | mob_id, meerkat_id, initial_message? | result JSON | async, retire + re-spawn same profile |
| `mob_events` | mob_id, after_cursor (u32), limit (u32) | MobEvent[] JSON | |
| `mob_status` | mob_id | JSON | |
| `mob_list` | | MobSummary[] JSON | |
| `mob_lifecycle` | mob_id, action string | `()` | async, stop/resume/complete/destroy |
| `mob_run_flow` | mob_id, flow_id, params JSON | run_id string | async |
| `mob_flow_status` | mob_id, run_id | MobRun JSON | |
| `mob_cancel_flow` | mob_id, run_id | `()` | async |
| `wire_cross_mob` | mob_a, meerkat_a, mob_b, meerkat_b | `()` | async, cross-mob comms wiring |

### Subscriptions
| Export | Params | Returns | Notes |
|--------|--------|---------|-------|
| `mob_member_subscribe` | mob_id, meerkat_id | handle (u32) | async, per-member broadcast subscription |
| `mob_subscribe_events` | mob_id | handle (u32) | async, mob-wide attributed event stream |
| `poll_subscription` | handle | JSON | Drain events from subscription |
| `close_subscription` | handle | `()` | Close subscription handle |

### Inspection
| Export | Params | Returns | Notes |
|--------|--------|---------|-------|
| `inspect_mobpack` | bytes | manifest JSON | No init needed |

### Comms
| Export | Params | Returns | Notes |
|--------|--------|---------|-------|
| `comms_peers` | session_id | JSON | List trusted peers |
| `comms_send` | session_id, params JSON | JSON | Send comms message |

## Config JSON Formats

### RuntimeConfig / Credentials
```json
{
  "api_key": "sk-...",
  "anthropic_api_key": "sk-...",
  "openai_api_key": "sk-...",
  "gemini_api_key": "sk-...",
  "model": "claude-sonnet-4-5",
  "max_sessions": 64,
  "base_url": "https://fallback-proxy.example.com",
  "anthropic_base_url": "https://proxy.example.com/anthropic",
  "openai_base_url": "https://proxy.example.com/openai",
  "gemini_base_url": "https://proxy.example.com/gemini"
}
```

Per-provider base URLs take precedence over `base_url`. Used for proxy deployments where API keys are injected server-side.

## State Architecture

```
thread_local! {
    REGISTRY: RefCell<RuntimeRegistry>              // Legacy session management
    RUNTIME_STATE: RefCell<Option<RuntimeState>>    // Service-based infrastructure
    SUBSCRIPTIONS: RefCell<SubscriptionRegistry>    // Event subscription handles
}

RuntimeState {
    mob_state: Arc<MobMcpState>,                    // All mob operations
    session_service: Arc<WasmSessionService>,       // Concrete service for subscriptions
    model: String,                                  // Default model
}
```

`MobMcpState::new(service)` wraps `EphemeralSessionService<FactoryAgentBuilder>`.
All mob operations create sessions through the same service.

## Mob Spawn Spec Format

```json
[
  {
    "profile": "planner",
    "meerkat_id": "planner-1",
    "runtime_mode": "turn_driven",
    "initial_message": "optional prompt",
    "additional_instructions": ["Extra context for this member"],
    "labels": { "role": "lead" },
    "context": { "custom": "data" }
  }
]
```

`runtime_mode`: `"turn_driven"` (explicit turns) or `"autonomous_host"` (requires comms event_injector).
`additional_instructions`: appended to the system prompt for this member only.

## MobDefinition JSON Format

```json
{
  "id": "my-mob",
  "profiles": {
    "planner": {
      "model": "claude-sonnet-4-5",
      "tools": { "comms": true },
      "peer_description": "Plans tasks",
      "skills": ["research"]
    },
    "operator": {
      "model": "claude-sonnet-4-5",
      "tools": { "comms": true, "mob_tasks": true },
      "peer_description": "Executes tasks"
    }
  },
  "wiring": {
    "auto_wire_orchestrator": false,
    "role_wiring": [{ "a": "planner", "b": "operator" }]
  },
  "flows": {
    "deliberate": {
      "steps": {
        "plan": { "role": "planner", "message": "..." },
        "validate": { "role": "operator", "message": "...", "depends_on": ["plan"] }
      }
    }
  }
}
```

Note: Profile has no `system_prompt` field — prompts are built from `skills` during agent construction.

## `@rkat/web` TypeScript API

The `@rkat/web` npm package provides a camelCase TypeScript wrapper:

```typescript
import { MeerkatRuntime } from '@rkat/web';
import * as wasm from '@rkat/web/wasm/meerkat_web_runtime.js';

// Register tools before init
MeerkatRuntime.registerTool(wasm, 'my_tool', 'desc', schema, callback);

// Initialize
const runtime = await MeerkatRuntime.init(wasm, {
  anthropicApiKey: 'sk-...',
  anthropicBaseUrl: 'http://localhost:3100/anthropic', // proxy
});

// Mob lifecycle
const mob = await runtime.createMob(definition);
await mob.spawn([{ profile: 'worker', meerkat_id: 'w1' }]);
const sub = mob.subscribe('w1');
const events = sub.poll();
sub.close();

// Direct sessions
const session = runtime.createSession({ model: '...', apiKey: '...' });
const result = await session.turn('Hello');
session.destroy();
```
