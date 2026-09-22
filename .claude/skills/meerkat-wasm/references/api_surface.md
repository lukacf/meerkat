# WASM Runtime API Surface

## wasm_bindgen Exports

(Verify against `meerkat-web-runtime/src/lib.rs` for the current tree; names
below are the exact JS-visible identifiers.)

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
| `register_js_tool` | name, description, schema JSON | `()` | Synchronous registration; dispatch reports pending detached host work, not completion; requires initialized runtime state |
| `clear_tool_callbacks` | — | `()` | Clear all registered JS tool callbacks |
| `register_external_auth_resolver` | callback (or `undefined` / `null` to clear) | `()` | Register a JS-side resolver that the agent factory calls to obtain a typed `ExternalAuthLease` for a given `authBinding`. Subsequent calls overwrite. Defined in `meerkat-web-runtime/src/external_auth.rs`. |
| `has_external_auth_resolver` | — | bool | Check whether a JS-side external auth resolver is registered |

For a fire-and-forget tool, the host observes `ToolCallRequested`, performs the
action asynchronously, and reports any actual completion/result through a later
session/mob message. Observing the request is not evidence that the action
completed.

### Session Lifecycle

| Export | Params | Returns | Notes |
|--------|--------|---------|-------|
| `create_session` | mobpack bytes, config JSON | handle (u32) | Trust-verifies the pack, then creates a session through the shared `WasmStandaloneSessionService` |
| `create_session_simple` | config JSON | handle (u32) | Same shared service; uses registered tools and any verified bootstrap pack's prompt |
| `start_turn` | handle, prompt JSON string | RunResult JSON | async; tagged `{"text": ...}` or `{"blocks": [...]}` input (no third options arg) |
| `append_system_context` | handle, request JSON | result JSON | async, append session system context |
| `get_session_state` | handle | JSON | Session metadata |
| `destroy_session` | handle | `()` | Remove session |
| `poll_events` | handle | AgentEvent[] JSON | Drain buffered direct-session events |

`config` for `create_session` / `create_session_simple` accepts an optional
`auth_binding` that scopes credential resolution to a realm/binding through the
provider runtime registry. If the selected binding uses the WASM external
resolver source, the registered resolver is invoked (see auth section). Stock
bootstrap only supplies inline-key bindings in `global`, not arbitrary realms
or external-resolver bindings.

The raw export requires tagged JSON even for text:

```typescript
await wasm.start_turn(handle, JSON.stringify({ text: 'Hello' }));
await wasm.start_turn(handle, JSON.stringify({ blocks: contentBlocks }));
```

In contrast, `Session.turn('Hello')` and `Session.turn(contentBlocks)` perform
that serialization in the SDK.

### Mob Lifecycle (delegates to MobMcpState)

| Export | Params | Returns | Notes |
|--------|--------|---------|-------|
| `mob_create` | definition JSON | mob_id string | async |
| `mob_status` | mob_id | JSON | async |
| `mob_list` | — | MobListResult JSON: `{mobs: [...]}` | async |
| `mob_lifecycle` | mob_id, action string | MobLifecycleResult JSON | async; stop/resume/complete/reset/destroy; includes `mob_id`, `action`, `ok`, and optional `destroy_report` |
| `mob_events` | mob_id, after_cursor (string), limit (u32) | MobEvent[] JSON | async |
| `mob_spawn` | mob_id, specs JSON | result JSON | async, batch spawn |
| `mob_retire` | mob_id, agent_identity | `()` | async |
| `mob_respawn` | mob_id, agent_identity, initial_message? | result JSON | async, retire + re-spawn same profile |
| `mob_force_cancel` | mob_id, agent_identity | `()` | async, force-cancel an in-flight turn |
| `mob_member_status` | mob_id, agent_identity | JSON | async, execution status snapshot |
| `mob_member_send` | mob_id, agent_identity, payload JSON | delivery receipt JSON | async; receipt keys: `mob_id`, `agent_identity`, `member_ref`, `handling_mode` (`queue` or `steer`) |
| `mob_member_peer_target` | mob_id, member | peer target JSON | async, resolve a member name to its canonical comms peer target; wrapped by `Member.peerTarget()` in `@rkat/web` |
| `mob_list_members` | mob_id | RosterEntry[] JSON | async |
| `mob_append_system_context` | mob_id, agent_identity, request JSON | result JSON | async, append context to a member's system prompt |
| `mob_wire` / `mob_unwire` | mob_id, a, b | `()` | async, identity-keyed wiring |
| `mob_wire_peer` / `mob_unwire_peer` | mob_id, member, peer JSON | `()` | async, canonical member/peer wiring (structured peer descriptor). There are no `mob_wire_target` / `wire_cross_mob` exports. |
| `mob_spawn_helper` | mob_id, request JSON (requires `result_label`, `max_text_bytes`) | result JSON | async, helper spawn with auto-wait |
| `mob_fork_helper` | mob_id, request JSON (requires `result_label`, `max_text_bytes`) | result JSON | async, fork-from-source helper spawn |
| `mob_run_flow` | mob_id, flow_id, params JSON | run_id string | async |
| `mob_flow_status` | mob_id, run_id | MobFlowStatusResult JSON: `{run: MobRun \| null}` | async |
| `mob_cancel_flow` | mob_id, run_id | `()` | async |

These raw JSON results are JavaScript strings. The SDK's `runtime.listMobs()`
unwraps `mobs`, and `mob.flowStatus()` unwraps `run` (or returns `null`).
`mob.lifecycle()` retains the parsed lifecycle result; inspect its
`destroy_report` rather than discarding the cleanup receipt.

### Subscriptions

| Export | Params | Returns | Notes |
|--------|--------|---------|-------|
| `mob_member_subscribe` | mob_id, agent_identity | stream_id (string) | async, per-member broadcast subscription |
| `mob_subscribe_events` | mob_id | stream_id (string) | async, mob-wide attributed event stream |
| `poll_subscription` | stream_id (string) | JSON | Drain events from subscription |
| `close_subscription` | stream_id (string) | `()` | Close subscription handle |

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

Browser-hosted authentication has three related APIs, but stock bootstrap
supports only the inline-key/proxy path:

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
   when a custom Rust/WASM bootstrap supplies a binding with
   `CredentialSourceSpec::ExternalResolver { handle: "wasm_host" }`.
3. **Per-runtime credentials** (init-only): `init_runtime` /
   `init_runtime_from_config` accept provider-specific keys and base URLs
   (`anthropic_api_key`, `openai_api_key`, `gemini_api_key`,
   `anthropic_base_url`, `openai_base_url`, `gemini_base_url`). The
   `@rkat/web` wrapper uses `anthropicApiKey` / `anthropicBaseUrl`,
   `openaiApiKey` / `openaiBaseUrl`, and `geminiApiKey` / `geminiBaseUrl`.
   Generic `apiKey` / `baseUrl` fields are deleted at both runtime and session
   boundaries; per-session credentials are not accepted.

`MeerkatRuntime.init()` / `initFromMobpack()` and their raw bootstrap exports
do not accept realm/binding configuration or read native realm files. They
synthesize `InlineSecret` bindings under `global`. A callback registration
neither creates a realm nor converts an existing inline binding to an external
source, and `withAuthBinding(authBinding, config)` only sets a selector. Use a
provider key or proxy sentinel for runnable stock-browser sessions.

The following fragment illustrates resolver registration for a **custom
composition**, not a complete external-auth setup using the stock npm bundle.
The custom bootstrap must install the external-source binding described above:

```typescript
import { registerExternalAuthResolver } from '@rkat/web';
import * as wasm from '@rkat/web/wasm/meerkat_web_runtime.js';

await wasm.default(); // Load the wasm-pack web binary before invoking an export.
registerExternalAuthResolver(wasm, async (authBinding) => {
  const token = await myHostFetchToken(authBinding);
  return {
    kind: 'inline_secret',
    secret: token.accessToken,
    metadata: { account_id: token.accountId },
    expires_at: token.expiresAt,
  };
});
```

Register before the externally bound session is built. First runtime
installation preserves a registration made after binary initialization;
replacing an existing runtime clears it, so re-register after that bootstrap.

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
  "system_prompt": "You are helpful.",
  "max_tokens": 4096,
  "comms_name": "browser-agent",
  "labels": { "surface": "web" },
  "additional_instructions": ["Be concise."],
  "app_context": { "tenant": "team-alpha" }
}
```

`SessionConfig` does not accept `api_key` or `base_url`; credentials come from
bootstrap-populated realm config or an existing selected `auth_binding`.
Omitting that selector uses the stock bootstrap's `global` bindings. External
bindings require the custom composition described above.

Direct Web sessions require explicit host-driven turns. Both
`"keep_alive": true` and `"keep_alive": false` are rejected with
`unsupported_session_option`; omit the field entirely. Mob
`runtime_mode: "autonomous_host"` is a separate in-memory member-host path,
not a direct-session keep-alive option.

## State Architecture

```
thread_local! {
    RUNTIME_STATE: RefCell<Option<RuntimeState>>    // Shared service and handle map
    SUBSCRIPTIONS: RefCell<SubscriptionRegistry>    // Event subscription handles
    EXTERNAL_AUTH_RESOLVER: RefCell<Option<Function>>
}

WasmStandaloneSessionService = EphemeralSessionService<FactoryAgentBuilder>

RuntimeState {
    mob_state: Arc<MobMcpState>,                         // All mob operations
    session_service: Arc<WasmStandaloneSessionService>,  // Direct sessions and mob members
    sessions: BTreeMap<u32, StandaloneHandleSession>,     // Browser-local handle map
    next_handle: u32,
    bootstrap_mobpack: Option<BootstrapMobpack>,          // Verified id + name + skills
    mobpack_trust: MobpackTrustConfig,                    // Bootstrap trust policy/store
    js_tools: Vec<JsToolEntry>,                          // wasm32 only
}

StandaloneHandleSession {
    session_id: SessionId,
    mob_id: String,
    event_rx: WasmSessionEventReceiver,
}
```

`MobMcpState::new(service, MobControlPrincipal::Owner)` wraps
the shared `WasmStandaloneSessionService` as an embedded, single-owner
substrate. Both direct handles and mob members use this
`EphemeralSessionService<FactoryAgentBuilder>`, whose builder injects a
`StoreAdapter` backed by `meerkat_store::MemoryStore`. A direct handle is not a
separate directly owned agent or a native runtime-backed session.

`destroy_runtime` clears the subscription registry and external auth resolver,
then drops `RuntimeState`, invalidating its browser-local handles. This is
in-memory teardown, not a durable cleanup receipt or a guarantee that
already-dispatched external host side effects have been cancelled.

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

Helper spawn/fork request JSON (`mob_spawn_helper`, `mob_fork_helper`) requires
`result_label` and `max_text_bytes` and may carry `auth_binding`; regular batch
`mob_spawn` specs carry neither. The helper result JSON requires `output`,
`tokens_used`, `agent_identity`, `member_ref`, `bounded_result`
(`label`/`status`/`text`), `session_id`, `usage`, `turns`, and `tool_calls`,
with optional `retirement_error`.

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
      "tools": { "comms": true, "builtins": true, "mob": true },
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

// Stock key/proxy bootstrap also loads the WASM binary.
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
const sessionEvents = session.subscribe();   // sync, returns EventSubscription<SessionEvent>
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
- `MobStatus` carries `mob_id` + `status` only; the deprecated `state` compatibility projection was deleted
- Per-session `apiKey` / `baseUrl` fields were removed; use runtime init-time provider keys/proxy URLs. `registerExternalAuthResolver` plus `authBinding` requires the custom bootstrap described in the auth section.
- Raw `start_turn` takes only `(handle, promptJson)`, where `promptJson` is `JSON.stringify({ text: 'Hello' })` or `JSON.stringify({ blocks: contentBlocks })`; there is no third options argument. SDK `Session.turn('Hello')` / `Session.turn(contentBlocks)` handles this encoding.
