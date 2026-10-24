# WASM Runtime API Surface

## wasm_bindgen Exports

(Verify against `crates/meerkat-web-runtime/src/lib.rs` for the current tree; names
below are the exact JS-visible identifiers.)

The `meerkat-web-runtime` crate exposes the browser bootstrap, session, mob,
auth, and subscription functions below via `#[wasm_bindgen]`. Names listed below
are the exact JS-visible identifiers in the Rust binding.

### Bootstrap

| Export | Params | Returns | Notes |
|--------|--------|---------|-------|
| `init_runtime` | mobpack bytes, credentials JSON | `Promise<string>` | Trust-verified mobpack bootstrap under the credentials' `mobpack_trust` (strict with an empty signer store by default); JSON result includes opaque `runtime_handle` |
| `init_runtime_from_config` | config JSON | `Promise<string>` | Bootstrap without a mobpack; JSON result includes opaque `runtime_handle` |
| `destroy_runtime` | optional expected runtime handle | `Promise<void>` | Await canonical teardown; a stale expected handle does not destroy a replacement runtime |
| `runtime_version` | — | version string | Returns `CARGO_PKG_VERSION` for JS/WASM version validation |
| `register_tool_callback` | name, description, schema JSON, callback | `()` | Register a promise-returning JS tool callback; it receives the arguments JSON string and resolves to `{"content": string \| ContentBlock[], "is_error": bool}` JSON; requires initialized runtime state |
| `register_js_tool` | name, description, schema JSON | `()` | Synchronous registration; dispatch reports pending detached host work, not completion; requires initialized runtime state |
| `clear_tool_callbacks` | — | `()` | Clear all registered JS tool callbacks |
| `register_external_auth_resolver` | callback (or `undefined` / `null` to clear) | `()` | Register a JS-side resolver that the agent factory calls to obtain a typed `ExternalAuthLease` for a given `authBinding`. Subsequent calls overwrite. Defined in `crates/meerkat-web-runtime/src/external_auth.rs`. |
| `has_external_auth_resolver` | — | bool | Check whether a JS-side external auth resolver is registered |

For a fire-and-forget tool, the host observes `ToolCallRequested`, performs the
action asynchronously, and reports any actual completion/result through a later
session/mob message. Observing the request is not evidence that the action
completed.

### Session Lifecycle

| Export | Params | Returns | Notes |
|--------|--------|---------|-------|
| `create_session` | mobpack bytes, config JSON | `Promise<number>` | Trust-verifies the pack, then materializes a canonical runtime-owned session and returns a local handle |
| `create_session_simple` | config JSON | `Promise<number>` | Same shared runtime helper; uses registered tools and any verified bootstrap pack's prompt |
| `start_turn` | handle, prompt JSON, optional options JSON | `Promise<string>` | Runtime admission and canonical `WireRunResult` JSON; prompt is tagged `{"text": ...}` or `{"blocks": [...]}` |
| `append_system_context` | handle, request JSON | `Promise<string>` | Canonical `InjectSystemContextResult` JSON containing `status` |
| `get_session_state` | handle | `Promise<string>` | Canonical `WireSessionInfo` JSON, including `session_id`; no browser handle, mob identity, or usage fields |
| `interrupt_session` | handle | `Promise<void>` | Interrupt through the shared machine authority |
| `stop_session_run` | handle, run_id, reason | `Promise<string>` | Run-fenced Stop of one exact run; canonical `WireRunStopReceipt` JSON (`outcome` is `stopped`, `not_current`, or `not_stoppable`); a stale run id is `not_current`, not an error |
| `session_wire_peer` | handle, peer handle | `Promise<void>` | Install directional in-process peer trust |
| `destroy_session` | handle | `Promise<void>` | Retire the exact runtime registration; remove the handle only after cleanup succeeds |
| `poll_events` | handle | AgentEvent[] JSON | Drain buffered canonical direct-session events; a lag gap arrives as `stream_truncated` |

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

`Session.turn('Hello')` and `Session.turn(contentBlocks)` perform that
serialization in the SDK. The optional third raw argument is serialized
`WireTurnInputOptions`, with `handling_mode`, `transient_turn_context`, and
`skill_references`; the SDK exposes these as `handlingMode`,
`transientTurnContext`, and `skillReferences`. Skill references are canonical
`SkillKey` objects, never bare skill-name strings. Unknown options fail closed.

```typescript
await wasm.start_turn(handle, JSON.stringify({ text: 'Hello' }), JSON.stringify({
  handling_mode: 'queue',
  transient_turn_context: 'Host facts for this turn only.',
}));
```

Successful turns return the generated `WireRunResult`, including
`terminal_cause_kind`. Agent/runtime faults reject with typed error codes;
there is no synthetic browser terminal-status payload. A retired direct handle
fails with `invalid_session_handle`, and failed cleanup retains the handle for
retry.

### Mob Lifecycle (delegates to MobMcpState)

| Export | Params | Returns | Notes |
|--------|--------|---------|-------|
| `mob_create` | definition JSON | mob_id string | async |
| `mob_status` | mob_id | MobStatusResult JSON: `{mob_id, status}` | async |
| `mob_list` | — | MobListResult JSON: `{mobs: [...]}` | async |
| `mob_lifecycle` | mob_id, action string | MobLifecycleResult JSON | async; stop/resume/complete/reset/destroy; includes `mob_id`, `action`, `ok`, and optional `destroy_report` (destroy) and `stop_report` (stop) |
| `mob_events` | mob_id, after_cursor (string), limit (u32) | MobEvent[] JSON | async |
| `mob_spawn` | mob_id, specs JSON | result JSON | async, batch spawn |
| `mob_retire` | mob_id, agent_identity | `()` | async |
| `mob_respawn` | mob_id, agent_identity, initial_message? (tagged prompt JSON, as for `start_turn`) | MobRespawnResult JSON | async, retire + re-spawn same profile; `status` is `completed` or `topology_restore_failed`, with `receipt` and `failed_peer_ids` |
| `mob_force_cancel` | mob_id, agent_identity | `()` | async, force-cancel an in-flight turn |
| `mob_stop_member_run` | mob_id, agent_identity, run_id, reason | `WireRunStopReceipt` JSON | async, run-fenced Stop of one exact member run; a stale run id is `not_current` |
| `mob_member_status` | mob_id, agent_identity | MobMemberStatusResult JSON | async, execution status snapshot with `member_ref` and tri-state `peer_connectivity` |
| `mob_member_send` | mob_id, agent_identity, request JSON (`content`, optional `handling_mode`, `render_metadata`) | delivery receipt JSON | async; receipt keys: `mob_id`, `agent_identity`, `member_ref`, `handling_mode` (`queue` or `steer`) |
| `mob_member_peer_target` | mob_id, member | peer target JSON | async, resolve a member to an external in-process `PeerTarget` (comms public key and address) for `mob_wire_peer` on another mob; wrapped by `Mob.crossMobPeerTarget(member)` and consumed by `Mob.wireToPeer(member, targetJson)` in `@rkat/web` |
| `mob_list_members` | mob_id | RosterEntry[] JSON | async |
| `mob_append_system_context` | mob_id, agent_identity, request JSON (`text`, optional `source`, `idempotency_key`) | `{mob_id, agent_identity, status}` JSON | async, append one ordinary durable System message to the member's session |
| `mob_wire` / `mob_unwire` | mob_id, a, b | `()` | async, identity-keyed wiring |
| `mob_wire_peer` / `mob_unwire_peer` | mob_id, member, peer JSON | `()` | async, canonical member/peer wiring (structured peer descriptor). There are no `mob_wire_target` / `wire_cross_mob` exports. |
| `mob_spawn_helper` | mob_id, request JSON (requires `prompt`, `agent_identity`, `result_label`, `max_text_bytes`) | result JSON | async, helper spawn with auto-wait |
| `mob_fork_helper` | mob_id, request JSON (requires `source_member_id`, `prompt`, `agent_identity`, `result_label`, `max_text_bytes`) | result JSON | async, fork-from-source helper spawn |
| `mob_run_flow` | mob_id, flow_id, params JSON | run_id string | async |
| `mob_flow_status` | mob_id, run_id | MobFlowStatusResult JSON: `{run: MobRun \| null}` | async |
| `mob_cancel_flow` | mob_id, run_id | `()` | async |

These raw JSON results are JavaScript strings. The SDK's `runtime.listMobs()`
unwraps `mobs`, and `mob.flowStatus()` unwraps `run` (or returns `null`).
`mob.lifecycle()` retains the parsed lifecycle result; inspect its
`destroy_report` or `stop_report` rather than discarding the cleanup receipt.
A rejected `mob.lifecycle()` is a `MeerkatError`. A destroy that did not
finish (here, or in `destroy_session` or `destroy_runtime`) rejects with code
`mob_destroy_incomplete`: the raw error JSON carries `destroy_report` and
`retryable` at its top level and the typed incomplete data
(`{code, destroy_report, retryable}`, as JSON-RPC's `error.data`) under
`data`, which `MeerkatError.fromWasm` keeps; `mobDestroyIncompleteReport(error)`
reads the report typed.

### Subscriptions

| Export | Params | Returns | Notes |
|--------|--------|---------|-------|
| `mob_member_subscribe` | mob_id, agent_identity | stream_id (string) | async, per-member broadcast subscription |
| `mob_subscribe_events` | mob_id | stream_id (string) | async, mob-wide attributed event stream |
| `poll_subscription` | stream_id (string) | JSON | Drain buffered events as an array: per-member `EventEnvelope<AgentEvent>` (a lag gap arrives as `stream_truncated`) or mob-wide `AttributedEvent` (`source`, `source_fence_token`, `role`, `envelope`); unknown ids fail with `invalid_stream_id` |
| `close_subscription` | stream_id (string) | `()` | Close subscription handle |

### Inspection

| Export | Params | Returns | Notes |
|--------|--------|---------|-------|
| `inspect_mobpack` | bytes | manifest JSON | No init needed; read-only, so no trust verification (consuming ingress verifies) |

### Comms

There are no current low-level `comms_peers` / `comms_send` wasm-bindgen
exports in `crates/meerkat-web-runtime/src/lib.rs`. Browser comms flow through
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
- Resolve to the structured lease object; a bare token string is refused. Reject with a typed `ExternalAuthFailure` (for example `{ kind: 'interactive_login_required' }` or `{ kind: 'refresh_failed', detail }`) to keep failure truth across the boundary.
- `sdks/web/examples/oauth-host.html` is a registration-only demo that reports the slot through `has_external_auth_resolver`.
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
  "gemini_base_url": "https://proxy.example.com/gemini",
  "required_capabilities": ["in_memory_persistence", "keep_alive", "comms"],
  "mobpack_trust": {
    "policy": "strict",
    "trusted_signers": { "signers": { "release@example.com": "<ed25519 public key hex>" } }
  }
}
```

Raw `init_runtime` / `init_runtime_from_config` require at least one
provider-specific key. Use a proxy sentinel such as `"proxy"` when the browser
calls a server-side provider proxy that injects the real credential.
`required_capabilities` is checked by the shared `BrowserRuntimeProfile`; the
SDK field is `requiredCapabilities`. An excluded requirement rejects with
`CAPABILITY_UNAVAILABLE` and typed `profile`, `capability`, and
`clearing_action` data. Unknown bootstrap and session fields are rejected.
`mobpack_trust` (SDK `mobpackTrust`, whose `trustedSigners` is the signer map)
is the trust store for every pack ingress; omitted means strict with no
signers. `max_sessions` defaults to 100000.

### SessionConfig

```json
{
  "model": "claude-sonnet-4-6",
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
bootstrap-populated realm config or an existing selected `auth_binding`.
Omitting that selector uses the stock bootstrap's `global` bindings. External
bindings require the custom composition described above.

Direct Web sessions use host-submitted turns by default. Set `keep_alive: true`
with `comms_name` to admit peer work between turns through the canonical runtime.
The SDK names are `keepAlive` and `commsName`. Set up directional trust with
`session_wire_peer(handle, peer_handle)` or `session.wirePeer(peer)`; establish
both directions when both agents need to receive from each other. Keep-alive
lasts while the page and runtime are alive. Mob `runtime_mode` selects member
hosting behavior on that same runtime authority.

## State Architecture

The following is a structural outline; source types live in
`crates/meerkat-web-runtime/src/lib.rs`:

```text
thread_local! {
    RUNTIME_STATE: RefCell<Option<RuntimeState>>
    RUNTIME_LIFECYCLE: Arc<Mutex<()>>           // Serializes install/create/teardown
    NEXT_SESSION_HANDLE: Cell<u32>            // Monotonic across runtime replacement
    SUBSCRIPTIONS: RefCell<SubscriptionRegistry>
    EXTERNAL_AUTH_RESOLVER: RefCell<Option<Function>>
}

WasmSessionService = EphemeralSessionService<FactoryAgentBuilder>

RuntimeState {
    instance_handle: String,
    mob_state: Arc<MobMcpState>,
    machine: Arc<MeerkatMachine>,
    session_service: Arc<WasmSessionService>,
    sessions: BTreeMap<u32, HandleSession>,
    bootstrap_mobpack: Option<BootstrapMobpack>,
    mobpack_trust: MobpackTrustConfig,
    js_tools: Vec<JsToolEntry>,                // wasm32 only
}

HandleSession {
    session_id: SessionId,
    registration: RuntimeSessionRegistrationWitness,
    event_rx: WasmSessionEventReceiver,
}
```

`MobMcpState::new(service, MobControlPrincipal::Owner)` and direct sessions share
one `EphemeralSessionService<FactoryAgentBuilder>` and its cached
`MeerkatMachine` adapter. The factory uses the browser capability profile and a
`StoreAdapter` backed by `MemoryStore`. Storage lifetime does not select a
second authority.

Direct creation lowers into the shared
`meerkat::surface::materialize_ephemeral_runtime_session` transaction, which
prepares runtime bindings, builds with `SessionOwned`, attaches the actor, and
installs peer ingress while retaining exact cancellation cleanup custody.
Turns and interruption lower into the shared runtime helpers. Local handles
and event buffers resolve canonical identity and project events only.

Await `destroy_runtime(expected_handle?)` for canonical session and mob
teardown. The SDK passes its bootstrap `runtime_handle`, so an old wrapper
cannot destroy a replacement runtime. Success clears subscriptions and the
external auth resolver, then drops `RuntimeState`. A cleanup failure retains
the remaining authority for retry. Page-lifetime storage and already-dispatched
external host side effects do not become durable or reversible through this
operation.

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

`runtime_mode`: `"turn_driven"` or `"autonomous_host"` (the default). Specs may
also carry `backend` (`"session"` or `"external"`; not exposed by the
`@rkat/web` `SpawnSpec`) and `model_override`. `initial_message` is plain
`ContentInput` (a string or a block array). Unknown fields are rejected, and a
`placement` fails with `CAPABILITY_UNAVAILABLE` (`remote_member_placement`,
clearing action `use_local_member_placement`).

Helper spawn/fork request JSON (`mob_spawn_helper`, `mob_fork_helper`) requires
`prompt`, `agent_identity`, `result_label`, and `max_text_bytes` (fork also
`source_member_id`) and may carry `role_name`, `model_override`,
`auth_binding`, `runtime_mode`, and `backend` (fork also `fork_context`,
default full history); regular batch `mob_spawn` specs carry no
`result_label`, `max_text_bytes`, or `auth_binding`. The helper result JSON
requires `output`, `tokens_used`, `agent_identity`, `member_ref`, `bounded_result`
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
      "tools": { "comms": true, "builtins": true },
      "peer_description": "Executes tasks"
    }
  },
  "skills": {
    "research": { "source": "inline", "content": "Check evidence before making a plan." }
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

Profile has no `system_prompt` field. Profile `skills` names select inline
content from the mob definition's `skills` table; verified mobpacks may also
supply embedded prompt content. For per-turn embedded skill selection, use
canonical `SkillKey` objects in `skillReferences`. These typed runtime skill
identities are distinct from a mob definition's inline-content names. A
`"source": "path"` skill needs runtime skill discovery and is refused
(`runtime_skills`). Browser mobs offer no agent mob-management tools, so
browser definitions leave `tools.mob` unset.

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
// Member-directed work; run-fenced stop takes a run_started event's identity.run_id
const receipt = await mob.member('w1').send('Summarize the plan.');
const stopped = await mob.stopMemberRun('w1', runId, 'superseded');

// Direct sessions
const session = await runtime.createSession({ model: 'claude-sonnet-4-6' });
const result = await session.turn('Hello', {
  transientTurnContext: 'Host facts for this turn only.',
});
const state = await session.getState();
const canonicalSessionId = await session.sessionId;
const sessionEvents = session.subscribe();   // sync, returns EventSubscription<SessionEvent>
sessionEvents.poll();
await session.destroy();
await runtime.destroy();
```

### Current API contracts

- `Mob.subscribeMemberEvents(agentIdentity)` and `Mob.subscribeEvents()` are **async** (return `Promise<EventSubscription<T>>`); `mob.member(id).subscribe()` is the per-member shorthand
- `EventSubscription<T>` is generic — `subscribeMemberEvents()` yields `MemberEventItem`, `subscribeEvents()` yields `AttributedEventItem`
- `Mob.events()` returns `MobEvent[]` (structural mob events, not agent events)
- `mob_create` and `mob_run_flow` return plain strings (not JSON-wrapped)
- `SpawnResult` is identity-native: `mob_id`, `agent_identity`, `member_ref`
- `MobMember` is identity-native and no longer exposes legacy bridge/session handle fields
- `MobStatus` carries `mob_id` + `status` only; the deprecated `state` compatibility projection was deleted
- Per-session `apiKey` / `baseUrl` fields were removed; use runtime init-time provider keys/proxy URLs. `registerExternalAuthResolver` plus `authBinding` requires the custom bootstrap described in the auth section.
- Raw `start_turn(handle, promptJson, optionsJson?)` accepts tagged `WirePromptInput` and optional `WireTurnInputOptions`. SDK `Session.turn(prompt, options?)` serializes both.
- `MeerkatRuntime.createSession`, `MeerkatRuntime.destroy`, `Session.getState`, `Session.destroy`, and `Session.sessionId` return promises. `Session.interrupt` and `Session.wirePeer` also await runtime authority.
- `SessionState` is generated from `WireSessionInfo`; `AppendSystemContextResult` exposes canonical `status` only. `TurnResult` is the generated `WireRunResult`, with no `response` alias or browser-only terminal status.
- Direct `keepAlive` is supported with `commsName`. Browser exclusions carry `CAPABILITY_UNAVAILABLE` and a typed clearing action from the shared capability profile.
- `Session.stopRun(runId, reason)` and `Mob.stopMemberRun(agentIdentity, runId, reason)` validate and return the `WireRunStopReceipt` union (`stopped`, `not_current`, `not_stoppable`).
- `Session.pollEvents()` and `Session.subscribe()` drain the same buffer; use one of them.
- `MeerkatError.fromWasm` wraps rejections from `createSession`, `runtime.destroy`, `Session.turn`/`getState`/`destroy`/`wirePeer`/`stopRun`/`interrupt`, and `Mob.spawn`/`respawn`/`stopMemberRun`/`spawnHelper`/`forkHelper`; other wrappers pass the raw WASM rejection through (normally a `{code, message}` JSON string). `Session.destroy()` treats `not_initialized` and `invalid_session_handle` as already gone.
