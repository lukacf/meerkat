# WASM Runtime API Surface

## wasm_bindgen Exports (current as of 0.6)

The `meerkat-web-runtime` crate exposes ~44 `#[wasm_bindgen]` functions across bootstrap, sessions, mobs, comms, auth, and subscriptions. Names listed below are the exact JS-visible identifiers.

### Bootstrap

| Export | Params | Returns | Notes |
|--------|--------|---------|-------|
| `init_runtime` | mobpack bytes, credentials JSON | init result JSON | Primary: mobpack-first bootstrap |
| `init_runtime_from_config` | config JSON | init result JSON | Bare-bones bootstrap |
| `destroy_runtime` | — | `()` | Tear down all runtime state (sessions, mob state, subscriptions) |
| `runtime_version` | — | version string | Returns `CARGO_PKG_VERSION` for JS/WASM version validation |
| `register_tool_callback` | name, description, schema JSON, callback | `()` | Must be called BEFORE init; legacy compat for registered tool callbacks |
| `register_js_tool` | name, description, schema JSON, callback | `()` | Newer JS tool registration entrypoint with promise-aware handling |
| `clear_tool_callbacks` | — | `()` | Clear all registered JS tool callbacks |
| `register_external_auth_resolver` | callback (or `undefined` to clear) | `()` | Register a JS-side resolver that the agent factory calls to obtain typed `AuthCredential` for a given `authBinding`. Subsequent calls overwrite. Defined in `meerkat-web-runtime/src/external_auth.rs`. |

### Session Lifecycle

| Export | Params | Returns | Notes |
|--------|--------|---------|-------|
| `create_session` | mobpack bytes, config JSON | handle (u32) | Direct AgentFactory path |
| `create_session_simple` | config JSON | handle (u32) | No mobpack, uses registered tool callbacks |
| `start_turn` | handle, prompt | RunResult JSON | async, single LLM turn (no third options arg) |
| `append_system_context` | handle, request JSON | result JSON | async, append session system context |
| `get_session_state` | handle | JSON | Session metadata |
| `destroy_session` | handle | `()` | Remove session |
| `poll_events` | handle | AgentEvent[] JSON | Drain buffered direct-session events |

`config` for `create_session_simple` accepts an optional `auth_binding` that scopes credential resolution to a realm/binding through the registered external auth resolver (see auth section).

### Mob Lifecycle (delegates to MobMcpState)

| Export | Params | Returns | Notes |
|--------|--------|---------|-------|
| `mob_create` | definition JSON | mob_id string | async |
| `mob_status` | mob_id | JSON | async |
| `mob_list` | — | MobSummary[] JSON | async |
| `mob_lifecycle` | mob_id, action string | `()` | async; stop/resume/complete/destroy |
| `mob_events` | mob_id, after_cursor (u32), limit (u32) | MobEvent[] JSON | async |
| `mob_spawn` | mob_id, specs JSON | result JSON | async, batch spawn |
| `mob_retire` | mob_id, agent_identity | `()` | async |
| `mob_respawn` | mob_id, agent_identity, initial_message? | result JSON | async, retire + re-spawn same profile |
| `mob_force_cancel` | mob_id, agent_identity | `()` | async, force-cancel an in-flight turn |
| `mob_member_status` | mob_id, agent_identity | JSON | async, execution status snapshot |
| `mob_member_send` | mob_id, agent_identity, payload JSON | `()` | async, send a message to a member |
| `mob_list_members` | mob_id | RosterEntry[] JSON | async |
| `mob_append_system_context` | mob_id, agent_identity, request JSON | result JSON | async, append context to a member's system prompt |
| `mob_wire` / `mob_unwire` | mob_id, a, b | `()` | async, identity-keyed wiring |
| `mob_wire_target` / `mob_unwire_target` | mob_id, local, target JSON | `()` | async, wire to a structured peer/target descriptor |
| `mob_wire_peer` / `mob_unwire_peer` | mob_id, member, peer JSON | `()` | async, peer-form wiring |
| `wire_cross_mob` | mob_a, identity_a, mob_b, identity_b | `()` | async, cross-mob comms wiring |
| `mob_spawn_helper` | mob_id, request JSON | result JSON | async, helper spawn with auto-wait |
| `mob_fork_helper` | mob_id, request JSON | result JSON | async, fork-from-source helper spawn |
| `mob_run_flow` | mob_id, flow_id, params JSON | run_id string | async |
| `mob_flow_status` | mob_id, run_id | MobRun JSON | async |
| `mob_cancel_flow` | mob_id, run_id | `()` | async |

### Subscriptions

| Export | Params | Returns | Notes |
|--------|--------|---------|-------|
| `mob_member_subscribe` | mob_id, agent_identity | handle (u32) | async, per-member broadcast subscription |
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

## Web SDK auth model

Browser-hosted authentication is done through three concepts:

1. **`authBinding`** (structural): every `runtime.createSession({...})`, `mob.spawn(...)`, etc. accepts an optional `authBinding` (realm + binding identifier). It scopes the agent build to a specific provider auth context, exactly the same way `--auth-binding` works on the CLI.
2. **`registerExternalAuthResolver`** (TS helper in `sdks/web/src/auth.ts`): wraps the wasm-bundled `register_external_auth_resolver` binding. The host page provides a function that maps a `AuthBindingRef` to a typed `AuthCredential` (bearer token, OAuth lease, etc.). The WASM agent factory calls this resolver instead of reading API keys.
3. **Per-runtime credentials** (init-only): `init_runtime` / `init_runtime_from_config` accept the legacy `anthropicApiKey` / `openaiApiKey` / `geminiApiKey` plus `*_base_url` overrides for proxy deployments. Per-session `apiKey` fields were removed in 0.6 — use the resolver pattern for anything more sophisticated than a global static key.

```typescript
import {
  MeerkatRuntime,
  registerExternalAuthResolver,
  withAuthBinding,
} from '@rkat/web';
import * as wasm from '@rkat/web/wasm/meerkat_web_runtime.js';

registerExternalAuthResolver(wasm, async (authBinding) => {
  const token = await myHostFetchToken(authBinding);
  return { kind: 'bearer_token', token };
});

const runtime = await MeerkatRuntime.init(wasm, {
  // legacy global keys still accepted, but resolver path is preferred:
  anthropicBaseUrl: 'https://my-proxy.example/anthropic',
});

// withAuthBinding(authBinding, config) returns a config with `authBinding` set;
// alternatively just set the field directly on the config object.
const session = runtime.createSession(withAuthBinding(
  authBinding,
  { model: 'claude-sonnet-4-6' },
));
```

Surface notes:

- The resolver is **session-build-time**, not request-time — once a session is built, the resolved lease is pinned for that build.
- `registerExternalAuthResolver(wasm, undefined)` (or passing `JsValue::NULL` directly to the WASM export) clears the registration.
- `authBinding` is also accepted on `mob.spawn(...)` member specs so an individual member can be bound to a different binding from its parent mob.

## Config JSON Formats

### RuntimeConfig / Credentials

```json
{
  "api_key": "sk-...",
  "anthropic_api_key": "sk-...",
  "openai_api_key": "sk-...",
  "gemini_api_key": "sk-...",
  "model": "claude-sonnet-4-6",
  "max_sessions": 64,
  "base_url": "https://fallback-proxy.example.com",
  "anthropic_base_url": "https://proxy.example.com/anthropic",
  "openai_base_url": "https://proxy.example.com/openai",
  "gemini_base_url": "https://proxy.example.com/gemini",
  "auth_binding": { "realm": "team-alpha", "binding": "claude" }
}
```

Per-provider base URLs take precedence over `base_url`. `auth_binding`, when present, is required to resolve through the registered external auth resolver.

## State Architecture

```
thread_local! {
    REGISTRY: RefCell<RuntimeRegistry>              // Direct session handles
    RUNTIME_STATE: RefCell<Option<RuntimeState>>    // Service-based infrastructure
    SUBSCRIPTIONS: RefCell<SubscriptionRegistry>    // Event subscription handles
    EXTERNAL_AUTH_RESOLVER: RefCell<Option<Function>>
}

RuntimeState {
    mob_state: Arc<MobMcpState>,                    // All mob operations
    session_service: Arc<WasmSessionService>,       // Concrete service for subscriptions
    model: String,                                  // Default model
}
```

`MobMcpState::new(service)` wraps `EphemeralSessionService<FactoryAgentBuilder>` as an embedded substrate. All mob operations create sessions through that same service, but runtime-owned surface semantics like `keep_alive` still belong to the hosting runtime layer.

`destroy_runtime` zeroes `RUNTIME_STATE` and `REGISTRY`, closes outstanding subscriptions, and clears the external auth resolver.

## Mob Spawn Spec Format

```json
[
  {
    "profile": "planner",
    "agent_identity": "planner-1",
    "runtime_mode": "turn_driven",
    "initial_message": "optional prompt",
    "additional_instructions": ["Extra context for this member"],
    "labels": { "role": "lead" },
    "context": { "custom": "data" },
    "auth_binding": { "realm": "team-alpha", "binding": "claude" }
  }
]
```

`runtime_mode`: `"turn_driven"` or `"autonomous_host"`.

## MobDefinition JSON Format

```json
{
  "id": "my-mob",
  "profiles": {
    "planner": {
      "model": "claude-sonnet-4-6",
      "tools": { "comms": true },
      "peer_description": "Plans tasks",
      "skills": ["research"]
    },
    "operator": {
      "model": "claude-sonnet-4-6",
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
import {
  MeerkatRuntime,
  registerExternalAuthResolver,
  withAuthBinding,
} from '@rkat/web';
import * as wasm from '@rkat/web/wasm/meerkat_web_runtime.js';

// Register tools before init
MeerkatRuntime.registerTool(wasm, 'my_tool', 'desc', schema, callback);

// Optional: register external auth resolver before any session build
registerExternalAuthResolver(wasm, async (ref) => ({ kind: 'bearer_token', token: '...' }));

// Initialize
const runtime = await MeerkatRuntime.init(wasm, {
  anthropicBaseUrl: 'http://localhost:3100/anthropic',
});

// Mob lifecycle
const mob = await runtime.createMob(definition);
await mob.spawn([{ profile: 'worker', agent_identity: 'w1' }]);
// Per-member subscription (async, EventSubscription<MemberEventItem>)
const sub = await mob.subscribeMemberEvents('w1');
const events = sub.poll();
sub.close();
// Or via the member object: const memberSub = await mob.member('w1').subscribe();
// Mob-wide attributed event stream (async, EventSubscription<AttributedEventItem>)
const mobWide = await mob.subscribeEvents();

// Direct sessions
const session = runtime.createSession({ model: 'claude-sonnet-4-6' });
const result = await session.turn('Hello');
const sessionEvents = session.subscribe();   // sync, returns EventSubscription<EventEnvelope>
sessionEvents.poll();
session.destroy();
```

### Key type changes (0.6)

- `Mob.subscribeMemberEvents(agentIdentity)` and `Mob.subscribeEvents()` are **async** (return `Promise<EventSubscription<T>>`); `mob.member(id).subscribe()` is the per-member shorthand
- `EventSubscription<T>` is generic — `subscribeMemberEvents()` yields `MemberEventItem`, `subscribeEvents()` yields `AttributedEventItem`
- `Mob.events()` returns `MobEvent[]` (structural mob events, not agent events)
- `mob_create` and `mob_run_flow` return plain strings (not JSON-wrapped)
- `SpawnResult` is identity-native: `agent_identity`, `agent_runtime_id`, `fence_token`, optional `generation`
- `MobMember` is identity-native and no longer exposes legacy bridge/session handle fields
- `MobStatus` uses `state` field instead of `status` + `member_count`
- Per-session `apiKey` fields were removed; use `runtime.init({ ... globalKeys })` and/or `registerExternalAuthResolver` plus `authBinding`
- `start_turn` now takes only `(handle, prompt)`; the legacy options-JSON third argument was removed
