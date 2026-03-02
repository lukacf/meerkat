# WASM Runtime API Surface

## 29 wasm_bindgen Exports

### Bootstrap
| Export | Params | Returns | Notes |
|--------|--------|---------|-------|
| `init_runtime` | mobpack bytes, credentials JSON | `()` | Primary: mobpack-first bootstrap |
| `init_runtime_from_config` | config JSON | `()` | Advanced: bare-bones bootstrap |

### Session Lifecycle
| Export | Params | Returns | Notes |
|--------|--------|---------|-------|
| `create_session` | mobpack bytes, config JSON | handle (u32) | Direct AgentFactory path |
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
| `mob_inject_and_subscribe` | mob_id, meerkat_id, message | interaction_id JSON | async, inject msg + subscribe |
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
    "initial_message": "optional prompt"
  }
]
```

`runtime_mode`: `"turn_driven"` (explicit turns) or `"autonomous_host"` (requires comms event_injector).

## MobDefinition JSON Format

```json
{
  "id": "my-mob",
  "profiles": {
    "planner": { "model": "claude-sonnet-4-5", "tools": { "comms": true } },
    "operator": { "model": "claude-sonnet-4-5", "tools": { "comms": true } }
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
