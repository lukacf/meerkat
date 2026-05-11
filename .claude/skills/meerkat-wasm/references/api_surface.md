# WASM Runtime API Surface

## wasm_bindgen Exports (current as of 0.6.5)

The `meerkat-web-runtime` crate exposes the browser bootstrap, session, mob,
auth, and subscription functions below via `#[wasm_bindgen]`. Names listed below
are the exact JS-visible identifiers in the Rust binding.

### Bootstrap

| Export | Params | Returns | Notes |
|--------|--------|---------|-------|
| `init_runtime` | mobpack bytes, credentials JSON | init result JSON | Primary: mobpack-first bootstrap |
| `init_runtime_from_config` | config JSON | init result JSON | Bare-bones bootstrap |
| `destroy_runtime` | — | `()` | Tear down all runtime state (sessions, mob state, subscriptions) |
| `runtime_version` | — | version string | Returns `CARGO_PKG_VERSION` for JS/WASM version validation |
| `register_tool_callback` | name, description, schema JSON, callback | `()` | Register a promise-returning JS tool callback; requires initialized runtime state |
| `register_js_tool` | name, description, schema JSON | `()` | Register a fire-and-forget JS tool that returns `"acknowledged"` immediately; requires initialized runtime state |
| `clear_tool_callbacks` | — | `()` | Clear all registered JS tool callbacks |
| `register_external_auth_resolver` | callback (or `undefined` / `null` to clear) | `()` | Register a JS-side resolver that the agent factory calls to obtain a typed `ExternalAuthLease` for a given `authBinding`. Subsequent calls overwrite. Defined in `meerkat-web-runtime/src/external_auth.rs`. |
| `has_external_auth_resolver` | — | bool | Check whether a JS-side external auth resolver is registered |

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

`config` for `create_session` / `create_session_simple` accepts an optional
`auth_binding` that scopes credential resolution to a realm/binding through the
provider runtime registry. If the selected binding uses the WASM external
resolver source, the registered resolver is invoked (see auth section).

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

There are no current low-level `comms_peers` / `comms_send` wasm-bindgen
exports in `meerkat-web-runtime/src/lib.rs`. Browser comms flow through
member-directed work (`mob_member_send`, `Member.send(...)`) and the comms tools
available to agents during turns.

## Web SDK auth model

Browser-hosted authentication is done through three concepts:

1. **`authBinding`** (structural): `runtime.createSession({...})`,
   `mob.spawnHelper(...)`, and `mob.forkHelper(...)` accept an optional
   `authBinding` (realm + binding identifier). It scopes the agent build to a
   specific provider auth context, exactly the same way `--auth-binding` works
   on the CLI. Plain `mob.spawn([...])` specs do not currently carry an auth
   binding in `@rkat/web`.
2. **`registerExternalAuthResolver`** (TS helper in `sdks/web/src/auth.ts`):
   wraps the wasm-bundled `register_external_auth_resolver` binding. The host
   page provides a function that maps an `AuthBindingRef` to a typed
   `ExternalAuthLease` (`inline_secret`, `static_headers`,
   `dynamic_authorizer`, or `none`). The WASM agent factory calls this resolver
   when the selected binding uses the WASM external-resolver credential source.
3. **Per-runtime credentials** (init-only): `init_runtime` /
   `init_runtime_from_config` accept provider-specific keys and base URLs
   (`anthropic_api_key`, `openai_api_key`, `gemini_api_key`,
   `anthropic_base_url`, `openai_base_url`, `gemini_base_url`). The
   `@rkat/web` wrapper still exposes `apiKey` / `baseUrl` compatibility fields,
   but current raw WASM config should use provider-specific fields. Per-session
   `apiKey` / `baseUrl` fields were removed in 0.6.

```typescript
import {
  MeerkatRuntime,
  registerExternalAuthResolver,
  withAuthBinding,
} from '@rkat/web';
import * as wasm from '@rkat/web/wasm/meerkat_web_runtime.js';

registerExternalAuthResolver(wasm, async (authBinding) => {
  const token = await myHostFetchToken(authBinding);
  return {
    kind: 'inline_secret',
    secret: token.accessToken,
    metadata: { account_id: token.accountId },
    expires_at: token.expiresAt,
  };
});

const runtime = await MeerkatRuntime.init(wasm, {
  anthropicApiKey: 'proxy',
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
- `clearExternalAuthResolver(wasm)` clears the registration; the raw WASM export also clears on `undefined` / `null`.
- Helper spawn paths accept `authBinding`; plain `mob.spawn([...])` does not currently include that field in the `@rkat/web` `SpawnSpec`.

## Config JSON Formats

### RuntimeConfig / Credentials

```json
{
  "anthropic_api_key": "sk-...",
  "openai_api_key": "sk-...",
  "gemini_api_key": "sk-...",
  "model": "claude-sonnet-4-6",
  "max_sessions": 64,
  "anthropic_base_url": "https://proxy.example.com/anthropic",
  "openai_base_url": "https://proxy.example.com/openai",
  "gemini_base_url": "https://proxy.example.com/gemini"
}
```

Raw `init_runtime` / `init_runtime_from_config` require at least one
provider-specific key. Use a proxy sentinel such as `"proxy"` when the browser
calls a server-side provider proxy that injects the real credential.

### SessionConfig

```json
{
  "model": "claude-sonnet-4-6",
  "auth_binding": { "realm": "team-alpha", "binding": "claude" },
  "system_prompt": "You are helpful.",
  "max_tokens": 4096,
  "comms_name": "browser-agent",
  "keep_alive": true,
  "labels": { "surface": "web" },
  "additional_instructions": ["Be concise."],
  "app_context": { "tenant": "team-alpha" }
}
```

`SessionConfig` does not accept `api_key` or `base_url`; credentials come from
bootstrap-populated realm config or from the selected `auth_binding`.

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
    sessions: BTreeMap<u32, RuntimeHandleSession>,  // Browser-local handles to runtime sessions
    next_handle: u32,
    js_tools: Vec<JsToolEntry>,                     // wasm32 only
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
    "context": { "custom": "data" }
  }
]
```

`runtime_mode`: `"turn_driven"` or `"autonomous_host"`.

Helper spawn/fork request JSON (`mob_spawn_helper`, `mob_fork_helper`) may carry
`auth_binding`; regular batch `mob_spawn` specs do not.

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

// Optional: register external auth resolver before any session build
registerExternalAuthResolver(wasm, async (ref) => ({
  kind: 'inline_secret',
  secret: await hostAuth.freshAccessToken(ref),
  metadata: {},
}));

// Initialize
const runtime = await MeerkatRuntime.init(wasm, {
  anthropicApiKey: 'proxy',
  anthropicBaseUrl: 'http://localhost:3100/anthropic',
});

// Register runtime-scoped tools after init
runtime.registerTool('my_tool', 'desc', schema, callback);
runtime.registerFireAndForgetTool('request_human_approval', 'desc', schema);

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
- `SpawnResult` is identity-native: `mob_id`, `agent_identity`, `member_ref`
- `MobMember` is identity-native and no longer exposes legacy bridge/session handle fields
- `MobStatus` uses `status`; `state` is a deprecated compatibility projection in `@rkat/web`
- Per-session `apiKey` / `baseUrl` fields were removed; use runtime init-time provider keys/proxy URLs and/or `registerExternalAuthResolver` plus `authBinding`
- `start_turn` now takes only `(handle, prompt)`; the legacy options-JSON third argument was removed
