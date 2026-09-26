# Meerkat TypeScript SDK

TypeScript client for the [Meerkat](https://github.com/lukacf/meerkat) runtime. The SDK is a thin session-first wrapper over the same runtime-backed contracts used by the CLI, REST, JSON-RPC, and MCP surfaces. It communicates with a local `rkat-rpc` subprocess over JSON-RPC 2.0 (newline-delimited JSON on stdin/stdout).

Current contract version: `0.8.40`.

## Installation

```bash
npm install @rkat/sdk
```

## Prerequisites

- **`rkat-rpc` binary on PATH** -- build the Meerkat repo through `make build`, then ensure the resulting `rkat-rpc` binary is in your `$PATH`. The SDK can also resolve and download a release binary automatically.
- **Node.js >= 18** (uses `node:child_process`, `node:readline`, `node:test`).
- **API key** for at least one LLM provider set in your environment (e.g. `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, or `GOOGLE_API_KEY`).

## tsconfig Requirements

The SDK is published as ESM (`"type": "module"` in package.json). Your project's `tsconfig.json` must use Node16 module resolution:

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "Node16",
    "moduleResolution": "Node16",
    "lib": ["ES2022"],
    "strict": true,
    "esModuleInterop": true
  }
}
```

## Quick Start

```ts
import { MeerkatClient } from "@rkat/sdk";

const client = new MeerkatClient();

// Connect spawns `rkat-rpc`, performs the initialize handshake,
// and fetches runtime capabilities.
await client.connect();

// Create a session (runs the first turn immediately).
const session = await client.createSession("What is the capital of Sweden?");

console.log(session.text);      // "Stockholm..."
console.log(session.id);        // UUID of the new session
console.log(session.usage);     // { inputTokens, outputTokens, ... }

// Multi-turn: send a follow-up in the same session.
const followUp = await session.turn("And what is its population?");
console.log(followUp.text);

// Clean up.
await session.archive();
await client.close();
```

## API Reference: MeerkatClient

### Constructor

```ts
new MeerkatClient(rkatPath?: string)
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `rkatPath` | `string` | `"rkat-rpc"` | Path to the RPC binary. |

### connect()

```ts
async connect(options?: ConnectOptions): Promise<this>
```

Spawns `rkat-rpc` as a child process, performs the `initialize` handshake, checks contract version compatibility, and fetches runtime capabilities via `capabilities/get`. Returns `this` for chaining.
Pass `{ liveWs: true }` only for WebSocket live-adapter flows that need
`live/*`; it requires a configured live provider. WebRTC uses
`{ liveWebrtc: true }` and a binary compiled with the non-default
`meerkat-rpc/live-webrtc` Cargo feature.

Throws `MeerkatError` with code `"VERSION_MISMATCH"` if the server's contract version is incompatible with the SDK's `CONTRACT_VERSION`.

### close()

```ts
async close(): Promise<void>
```

Kills the `rkat-rpc` subprocess and cleans up resources.

### createSession(prompt, options?)

```ts
async createSession(
  prompt: string | ContentBlock[],
  options?: SessionOptions,
): Promise<Session>
```

Creates a new session and immediately runs the first turn with the given prompt. Returns a runtime-backed `Session` wrapper whose `lastResult` tracks the latest `RunResult` and whose methods (`turn`, `stream`, `history`, `archive`) stay aligned with canonical backend semantics.

**Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `prompt` | `string \| ContentBlock[]` | **(required)** | The user prompt for the first turn. |
| `options.injectedContext` | `readonly ContentInput[]` | `undefined` | Ordered durable host-attached context immediately before the prompt. |
| `options.transientTurnContext` | `string` | `undefined` | Request-only host facts for the first turn. |
| `options.model` | `string` | Server/config default | LLM model name (e.g. `"gpt-5.5"`, `"gemini-3.5-flash"`, `"claude-opus-4-8"`). |
| `options.provider` | `string` | Resolved from the model registry | Force a specific provider (`"anthropic"`, `"openai"`, `"gemini"`). |
| `options.authBinding` | `WireAuthBindingRef` | `undefined` | Resolve credentials through an explicit realm binding. |
| `options.systemPrompt` | `string \| { action: "disable" }` | `undefined` | Override or explicitly disable inherited system prompts. |
| `options.maxTokens` | `number` | `undefined` | Maximum output tokens for the LLM response. |
| `options.outputSchema` | `Record<string, unknown>` | `undefined` | JSON Schema for structured output extraction. |
| `options.structuredOutputRetries` | `number` | `2` (server default) | Max retries for structured output validation. |
| `options.hooksOverride` | `Record<string, unknown>` | `undefined` | Run-scoped hook overrides. |
| `options.enableBuiltins` | `boolean` | Runtime/config default | Override built-in tool availability. |
| `options.enableShell` | `boolean` | Runtime/config default | Override shell-tool availability; enabling it also requires built-ins. |
| `options.enableMemory` | `boolean` | Runtime/config default | Override semantic-memory availability. |
| `options.enableSchedule` | `boolean` | Runtime/config default | Override schedule-tool availability. Omission currently inherits the configured runtime default. |
| `options.enableWorkGraph` | `boolean` | Runtime/config default | Override WorkGraph-tool availability. |
| `options.enableMob` | `boolean` | Runtime/config default | Override mob-orchestration helper availability. |
| `options.enableWebSearch` | `boolean` | Runtime/config default | Override the built-in web-search path. |
| `options.toolFilter` | `WireToolFilter` | `undefined` | Apply the session tool visibility policy. |
| `options.keepAlive` | `boolean` | `false` | Run in keep-alive mode for inter-agent comms. |
| `options.commsName` | `string` | `undefined` | Agent name for comms (required when `keepAlive` is `true`). |
| `options.peerMeta` | `Record<string, unknown>` | `undefined` | Metadata advertised to peer comms surfaces. |
| `options.budgetLimits` | `Record<string, unknown>` | `undefined` | Runtime budget limits for the session. |
| `options.providerParams` | `Record<string, unknown>` | `undefined` | Provider-specific parameters (e.g. thinking config). |
| `options.preloadSkills` | `SkillRef[]` | `undefined` | Structured skills to load before the run. |
| `options.skillRefs` | `SkillRef[]` | `undefined` | Canonical structured skill references. |
| `options.labels` | `Record<string, string>` | `undefined` | Session labels used for filtering and metadata. |
| `options.additionalInstructions` | `string[]` | `undefined` | Extra instruction blocks appended to the system prompt. |
| `options.appContext` | `unknown` | `undefined` | Opaque app context passed to custom builders. |
| `options.shellEnv` | `Record<string, string>` | `undefined` | Per-session shell environment variables. |
| `options.externalTools` | `Record<string, unknown>[]` | `undefined` | Inline callback tool definitions for this session. |

### createDeferredSession(prompt, options?)

```ts
async createDeferredSession(
  prompt: string | ContentBlock[],
  options?: SessionOptions,
): Promise<DeferredSession>
```

The server rejects `options.transientTurnContext` on deferred create because no
immediate turn exists. Current `DeferredTurnOptions` also omits
`injectedContext`, `transientTurnContext`, and `selfHostedServerId`. Set
injected context during deferred creation; transient first-turn context and a
self-hosted server route are not expressible through this wrapper.

Creates a session identity without running the first turn yet. Use
`await deferred.startTurn(...)` for a result or `deferred.stream(...)` for an
`EventStream` when you want the first runtime-backed turn to happen later.

### listSessions()

```ts
async listSessions(options?: {
  labels?: Record<string, string>;
  limit?: number;
  offset?: number;
}): Promise<SessionInfo[]>
```

Returns an array of typed `SessionInfo` objects with camelCase fields such as `sessionId`, `sessionRef`, `messageCount`, `isActive`, and `labels`.

### readSession(sessionId)

```ts
async readSession(sessionId: string): Promise<SessionInfo>
```

Returns typed session details including `model`, `provider`, `lastAssistantText`, and `labels`.

### Session ingress helpers

```ts
async sendExternalEvent(sessionId: string, eventType: string, payload: unknown, options?: { blocks?: ContentBlock[] }): Promise<Record<string, unknown>>
async injectContext(sessionId: string, text: string, options?: { source?: string; idempotencyKey?: string }): Promise<{ status: string }>
```

These expose `session/external_event` and `session/inject_context` as first-class public APIs.

Input reconciliation, ATIF export, transcript revision read/list, forking,
guarded rewriting, system-prompt update, and revision restore are also public
through the corresponding `inputState`, `exportSessionAtif`, and
`*SessionTranscript*` helpers.

`Session.turn` and `Session.stream` accept
`injectedContext`, `transientTurnContext`, `skillRefs`, `turnToolOverlay`,
`additionalInstructions`, `keepAlive`, `model`, `provider`,
`selfHostedServerId`, `maxTokens`, `systemPrompt`, `outputSchema`,
`structuredOutputRetries`, and `providerParams`.

`DeferredSession.startTurn` and `DeferredSession.stream` accept `skillRefs`,
`turnToolOverlay`, `additionalInstructions`, `keepAlive`, `model`, `provider`,
`maxTokens`, `systemPrompt`, `outputSchema`, `structuredOutputRetries`, and
`providerParams`.

### Schedules, Models, and Mob profile APIs

```ts
async getModelsCatalog(): Promise<ModelsCatalog>

async createSchedule(request: CreateScheduleRequest): Promise<Schedule>
async getSchedule(scheduleId: string): Promise<Schedule>
async listSchedules(options?: ScheduleListOptions): Promise<Schedule[]>
async updateSchedule(request: UpdateScheduleRequest): Promise<Schedule>
async pauseSchedule(scheduleId: string): Promise<Schedule>
async resumeSchedule(scheduleId: string): Promise<Schedule>
async deleteSchedule(scheduleId: string): Promise<Schedule>
async listScheduleOccurrences(scheduleId: string, options?: ScheduleOccurrencesOptions): Promise<ScheduleOccurrencesResult>
async listScheduleTools(): Promise<ScheduleToolsResult>
async callScheduleTool(request: ScheduleToolCallRequest): Promise<Record<string, unknown>>

// WorkGraph is read-only from the SDK; agents mutate it through WorkGraph tools.
async getWorkGraphItem(itemId: string, options?: WorkGraphItemLookupOptions): Promise<WorkItem>
async listWorkGraphItems(filter?: WorkItemFilter): Promise<WorkItemsResult>
async listReadyWorkGraphItems(filter?: ReadyWorkFilter): Promise<WorkItemsResult>
async getWorkGraphSnapshot(filter?: WorkGraphSnapshotFilter): Promise<WorkGraphSnapshot>
async listWorkGraphEvents(filter?: WorkGraphEventFilter): Promise<WorkEventsResult>
async getWorkGraphGoalStatus(params: GoalStatusRequest): Promise<GoalStatusResult>
async listWorkGraphAttention(params?: AttentionListRequest): Promise<AttentionListResult>

async readMobEvents(mobId: string, options?: MobEventsOptions): Promise<MobEventsResult>
async spawnMobMembers(mobId: string, specs: SpawnSpec[]): Promise<MobSpawnManyResultEntry[]>

async createMobProfile(name: string, profile: MobProfile): Promise<MobProfileLookupResult>
async getMobProfile(name: string): Promise<MobProfileLookupResult>
async listMobProfiles(): Promise<MobProfileLookupResult[]>
async updateMobProfile(name: string, profile: MobProfile, expectedRevision: number): Promise<MobProfileLookupResult>
async deleteMobProfile(name: string, expectedRevision: number): Promise<MobProfileDeleteResult>
```

The client additionally exposes detached jobs and monitors, approval and
artifact lifecycles, projected event cursor replay/snapshots, and trusted
multi-host mob controls. Every public TypeScript RPC wrapper binds its request
and response transport boundary to generated RPC-schema contracts. Generated
job/approval/artifact/event names are not re-exported from the package root in
0.8.24; pass matching object literals. Multi-host host/grant mutations are
trusted operator APIs and cannot be invoked through agent mob tools.

Event cursor and snapshot methods require host event projection. Explicitly
ephemeral memory realms do not expose those replay APIs.

Approval calls maintain audit records only; they do not automatically gate,
authorize, or execute an action. They persist when the RPC bundle exposes a
store path and otherwise remain process-local.

Auth profiles and credentials are realm-scoped. Provisioning returns an auth
binding for `SessionOptions.authBinding` or a mob member spec, so the spec does
not carry the provider secret. Remote hosts resolve bindings only inside their
authorized realm.

Live channels support both WebSocket and WebRTC. Branch on
`opened.transport.transport`; only `websocket` has `url`. Use
`client.liveWebrtcAnswer` for the canonical WebRTC SDP exchange.
The Node SDK does not provide `RTCPeerConnection`; inject a compatible
implementation or forward browser-created SDP to the Node process. Add an
audio track or the `meerkat.live` data channel, set the local description,
wait for ICE gathering to complete, and send `peer.localDescription.sdp`.
The 0.8.24 `answerLiveWebrtcOffer` helper does not wait for ICE completion and
sends the original offer SDP, so it is not the canonical browser exchange.
The 0.8.24 release binary omits the non-default `live-webrtc` Cargo feature,
so WebRTC requires a custom `rkat-rpc` build, for example:

```bash
./scripts/repo-cargo build -p meerkat-rpc --features live-webrtc
```

### Session lifecycle on wrappers

Cancellation and archive operations live on the runtime-backed session wrappers:

```ts
await session.interrupt();
await session.archive();
const history = await session.history({ limit: 20 });
```

### capabilities

```ts
client.capabilities
```

Returns the cached `Capability[]` collected during `connect()`.

### hasCapability(capabilityId)

```ts
hasCapability(capabilityId: string): boolean
```

Returns `true` if the given capability is `"Available"` in the runtime. Known capability IDs:

| Capability ID | Description |
|---------------|-------------|
| `"sessions"` | Session lifecycle (create/turn/list/read/archive) |
| `"streaming"` | Real-time event streaming |
| `"structured_output"` | JSON schema-based structured output extraction |
| `"hooks"` | Lifecycle hooks |
| `"builtins"` | Built-in tools |
| `"shell"` | Shell tool |
| `"comms"` | Inter-agent communication |
| `"memory_store"` | Semantic memory |
| `"session_store"` | Session persistence |
| `"session_compaction"` | Context compaction |
| `"skills"` | Skill loading and invocation |

### requireCapability(capabilityId)

```ts
requireCapability(capabilityId: string): void
```

Throws `MeerkatError` with code `"CAPABILITY_UNAVAILABLE"` if the capability is not available.

### getConfig()

```ts
async getConfig(): Promise<ConfigEnvelope>
```

Returns the generated `ConfigEnvelope`: `config: unknown`, `generation: number`,
and optional nullable `realm_id`, `instance_id`, `backend`, and `resolved_paths`.
These metadata keys retain their snake_case wire spelling.

### setConfig(config)

```ts
async setConfig(config: Record<string, unknown>): Promise<ConfigWriteResult>
```

Replaces the entire runtime configuration and returns the updated envelope plus any live-propagation report.

### patchConfig(patch)

```ts
async patchConfig(patch: Record<string, unknown>): Promise<ConfigWriteResult>
```

Merge-patches the runtime configuration and returns the updated envelope plus any live-propagation report.

## Public Types

The TypeScript SDK exposes camelCase domain types at the package root.
Generated results such as `ConfigEnvelope` and `ConfigWriteResult` retain
snake_case wire keys; config writes add optional nullable `live_propagation`.
Their `config` value is `unknown`: validate it before spreading it or reading
individual properties.

- `RunResult` is available from `session.lastResult`, `await session.turn(...)`, `await deferred.startTurn(...)`, and `stream.result`.
- `Usage`, `SessionInfo`, `Capability`, and `SchemaWarning` model the runtime responses directly.
- `Session` and `DeferredSession` are the canonical runtime-backed wrappers for session lifecycle.
- `EventStream` yields typed events such as `text_delta`, `turn_completed`, and `tool_execution_completed`.

`TurnCompletedEvent.usage` is optional; absent means unmeasured, not zero.
`RunStartedEvent.input` preserves the typed `RunInput`: content (text or blocks)
or a `pending_tool_results` continuation without a fabricated prompt.
`RunFailedEvent` accepts the current `error_report` and derives its display
fields from that report.

Inventory-known events without a handwritten parser case arrive as
`UnknownEvent`, preserving `event.type` and the complete wire payload as fields
on the event. This includes `server_tool_content`, all four `model_fallback_*`
events, `transcript_rewrite_audit_receipt_committed`, `boundary_append_applied`,
and `boundary_appends_discarded`. A present type outside the generated inventory
raises `MeerkatError` with code `UNKNOWN_EVENT_TYPE`.

A missing type or malformed payload for a handwritten parser case arrives as
`MalformedEvent` (`type: "malformed_event"`), with the original payload in
`event.raw`. The `RetryingEvent` and `HookFailedEvent` parsers still expect
legacy flat fields; current payloads carry `retry` and `reason`, respectively,
so they take this malformed path. See the [event compatibility
reference](../../docs/sdks/typescript/reference.mdx#typed-events) for details.

Use the built-in client helpers directly for capability and skill flows:

```ts
import { MeerkatClient } from "@rkat/sdk";

const client = new MeerkatClient();

try {
  await client.connect();
  if (client.hasCapability("skills")) {
    const matches = (await client.listSkills()).filter(
      (entry) => entry.is_active && entry.key.skill_name === "code-review",
    );
    const [skill] = matches;
    if (!skill || matches.length !== 1) {
      throw new Error("Expected one active code-review skill; select a source UUID if ambiguous.");
    }
    const session = await client.createSession("Review this function");
    const result = await session.invokeSkill(
      { sourceUuid: skill.key.source_uuid, skillName: skill.key.skill_name },
      "Focus on performance regressions.",
    );
    console.log(result.text);
  }
} finally {
  await client.close();
}
```

Capability availability does not install a skill. Select an active entry from
`listSkills()` and map its wire-shaped key to the SDK's `SkillRef`; if the
name exists in multiple sources, choose the intended source UUID explicitly.

## Error Handling

The SDK provides a hierarchy of error classes, all extending the base `MeerkatError`:

### MeerkatError

```ts
class MeerkatError extends Error {
  readonly code: string;
  readonly details?: unknown;
  readonly capabilityHint?: {
    capability_id: string;
    message: string;
  };

  constructor(
    code: string,
    message: string,
    details?: unknown,
    capabilityHint?: { capability_id: string; message: string },
  );
}
```

Base error class. The `code` field contains a machine-readable error code (e.g. `"VERSION_MISMATCH"`, `"NOT_CONNECTED"`, `"CAPABILITY_UNAVAILABLE"`). The optional `capabilityHint` suggests which capability needs to be enabled.

### CapabilityUnavailableError

```ts
class CapabilityUnavailableError extends MeerkatError {}
```

Thrown when a required capability is not available in the runtime (e.g. feature not compiled in, disabled by policy).

### SessionNotFoundError

```ts
class SessionNotFoundError extends MeerkatError {}
```

Thrown when a session ID does not exist in the runtime.

### SkillNotFoundError

```ts
class SkillNotFoundError extends MeerkatError {}
```

Thrown when a referenced skill cannot be found.

### Error handling example

```ts
import {
  MeerkatClient,
  MeerkatError,
  CapabilityUnavailableError,
} from "@rkat/sdk";

const client = new MeerkatClient();

try {
  await client.connect();
  const session = await client.createSession("Hello");
  console.log(session.text);
} catch (err) {
  if (err instanceof CapabilityUnavailableError) {
    console.error("Missing capability:", err.message);
    if (err.capabilityHint) {
      console.error("Hint:", err.capabilityHint.message);
    }
  } else if (err instanceof MeerkatError) {
    console.error(`Meerkat error [${err.code}]: ${err.message}`);
  } else {
    throw err;
  }
} finally {
  await client.close();
}
```

## Version Compatibility

The SDK exports `CONTRACT_VERSION` (currently `"0.8.40"`). During `connect()`, the SDK checks that the server's contract version is compatible:

- While the major version is `0`, minor versions must match exactly (e.g. SDK `0.1.x` requires server `0.1.x`).
- Once `1.0.0` is reached, major versions must match (standard semver).

```ts
import { CONTRACT_VERSION } from "@rkat/sdk";
console.log(CONTRACT_VERSION);  // "0.8.40"
```

If the versions are incompatible, `connect()` throws a `MeerkatError` with code `"VERSION_MISMATCH"`.

## Config Management Example

```ts
const client = new MeerkatClient();
await client.connect();

// Read the current config.
const config = await client.getConfig();
console.log(config.generation, config.realm_id, config.config);

// Merge-patch a specific field without spreading the unknown config value.
const updated = await client.patchConfig(
  { max_tokens: 8192 },
  { expectedGeneration: config.generation },
);
console.log(updated.config);

await client.close();
```

## Structured Output Example

```ts
const client = new MeerkatClient();
await client.connect();

const session = await client.createSession("List three European capitals", {
  outputSchema: {
    type: "object",
    properties: {
      capitals: {
        type: "array",
        items: { type: "string" },
      },
    },
    required: ["capitals"],
  },
  structuredOutputRetries: 3,
});

// Parsed structured output (matches the schema).
console.log(session.structuredOutput);
// { capitals: ["Paris", "Berlin", "Madrid"] }

// Schema warnings from provider-specific validation issues.
if (session.lastResult.schemaWarnings) {
  for (const w of session.lastResult.schemaWarnings) {
    console.warn(`[${w.provider}] ${w.path}: ${w.message}`);
  }
}

await client.close();
```

## Multi-turn Conversation Example

```ts
const client = new MeerkatClient();
await client.connect();

// Create session with first turn.
const session = await client.createSession("My name is Alice.", {
  model: "claude-sonnet-4-6",
});

// Follow-up turns reuse the runtime-backed session handle.
const turn2 = await session.turn("What is my name?");
console.log(turn2.text);  // Should mention "Alice"

// Check session state.
const state = await client.readSession(session.id);
console.log(state);

// List all active sessions.
const sessions = await client.listSessions();
console.log(`Active sessions: ${sessions.length}`);

// Clean up.
await session.archive();
await client.close();
```

## License

MIT OR Apache-2.0
