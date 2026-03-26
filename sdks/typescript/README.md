# Meerkat TypeScript SDK

TypeScript client for the [Meerkat](https://github.com/lukacf/meerkat) runtime. The SDK is a thin session-first wrapper over the same runtime-backed contracts used by the CLI, REST, JSON-RPC, and MCP surfaces. It communicates with a local `rkat-rpc` subprocess over JSON-RPC 2.0 (newline-delimited JSON on stdin/stdout).

Current contract version: `0.5.0`.

## Installation

```bash
npm install @rkat/sdk
```

## Prerequisites

- **`rkat-rpc` binary on PATH** -- build it from the Meerkat repo with `cargo build -p meerkat-rpc`, then ensure the resulting `rkat-rpc` binary is in your `$PATH`.
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
async connect(): Promise<this>
```

Spawns `rkat-rpc` as a child process, performs the `initialize` handshake, checks contract version compatibility, and fetches runtime capabilities via `capabilities/get`. Returns `this` for chaining.

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
| `options.model` | `string` | Server default (typically `claude-sonnet-4-5`) | LLM model name (e.g. `"gpt-5.2"`, `"gemini-3-flash-preview"`, `"claude-opus-4-6"`). |
| `options.provider` | `string` | Auto-detected from model | Force a specific provider (`"anthropic"`, `"openai"`, `"gemini"`). |
| `options.systemPrompt` | `string` | `undefined` | Override the default system prompt. |
| `options.maxTokens` | `number` | `undefined` | Maximum output tokens for the LLM response. |
| `options.outputSchema` | `Record<string, unknown>` | `undefined` | JSON Schema for structured output extraction. |
| `options.structuredOutputRetries` | `number` | `2` (server default) | Max retries for structured output validation. |
| `options.hooksOverride` | `Record<string, unknown>` | `undefined` | Run-scoped hook overrides. |
| `options.enableBuiltins` | `boolean` | `false` | Enable built-in tools (task management, etc.). |
| `options.enableShell` | `boolean` | `false` | Enable the shell tool (requires `enableBuiltins`). |
| `options.enableMemory` | `boolean` | `false` | Enable semantic memory (memory_search tool + compaction indexing). |
| `options.enableMob` | `boolean` | `false` | Enable mob orchestration helpers. |
| `options.keepAlive` | `boolean` | `false` | Run in keep-alive mode for inter-agent comms. |
| `options.commsName` | `string` | `undefined` | Agent name for comms (required when `keepAlive` is `true`). |
| `options.peerMeta` | `Record<string, unknown>` | `undefined` | Metadata advertised to peer comms surfaces. |
| `options.budgetLimits` | `Record<string, unknown>` | `undefined` | Runtime budget limits for the session. |
| `options.providerParams` | `Record<string, unknown>` | `undefined` | Provider-specific parameters (e.g. thinking config). |
| `options.preloadSkills` | `string[]` | `undefined` | Skill source UUIDs to load before the run. |
| `options.skillRefs` | `SkillRef[]` | `undefined` | Canonical structured skill references. |
| `options.skillReferences` | `string[]` | `undefined` | Legacy string skill references; prefer `skillRefs`. |

### createDeferredSession(prompt, options?)

```ts
async createDeferredSession(
  prompt: string | ContentBlock[],
  options?: SessionOptions,
): Promise<DeferredSession>
```

Creates a session identity without running the first turn yet. Use `await deferred.startTurn(...)` when you want the first runtime-backed turn to happen later.

### listSessions()

```ts
async listSessions(): Promise<SessionInfo[]>
```

Returns an array of `SessionInfo` objects with camelCase fields such as `sessionId`, `sessionRef`, `messageCount`, and `isActive`.

### readSession(sessionId)

```ts
async readSession(sessionId: string): Promise<Record<string, unknown>>
```

Returns the current state payload for a session.

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
async getConfig(): Promise<Record<string, unknown>>
```

Returns the current Meerkat configuration as a JSON object.

### setConfig(config)

```ts
async setConfig(config: Record<string, unknown>): Promise<void>
```

Replaces the entire runtime configuration.

### patchConfig(patch)

```ts
async patchConfig(patch: Record<string, unknown>): Promise<Record<string, unknown>>
```

Merge-patches the runtime configuration and returns the resulting config.

## Public Types

The TypeScript SDK exposes camelCase domain types at the package root. The JSON-RPC snake_case wire format stays internal.

- `RunResult` is available from `session.lastResult`, `await session.turn(...)`, `await deferred.startTurn(...)`, and `stream.result`.
- `Usage`, `SessionInfo`, `Capability`, and `SchemaWarning` model the runtime responses directly.
- `Session` and `DeferredSession` are the canonical runtime-backed wrappers for session lifecycle.
- `EventStream` yields typed events such as `text_delta`, `turn_completed`, and `tool_execution_completed`.

Use the built-in client helpers directly for capability and skill flows:

```ts
const client = new MeerkatClient();
await client.connect();

if (client.hasCapability("skills")) {
  const session = await client.createSession("Review this function");
  const result = await session.invokeSkill(
    { sourceUuid: "source-123", skillName: "code-review" },
    "Focus on performance regressions.",
  );
  console.log(result.text);
}
```

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

The SDK exports `CONTRACT_VERSION` (currently `"0.4.11"`). During `connect()`, the SDK checks that the server's contract version is compatible:

- While the major version is `0`, minor versions must match exactly (e.g. SDK `0.1.x` requires server `0.1.x`).
- Once `1.0.0` is reached, major versions must match (standard semver).

```ts
import { CONTRACT_VERSION } from "@rkat/sdk";
console.log(CONTRACT_VERSION);  // "0.4.11"
```

If the versions are incompatible, `connect()` throws a `MeerkatError` with code `"VERSION_MISMATCH"`.

## Config Management Example

```ts
const client = new MeerkatClient();
await client.connect();

// Read the current config.
const config = await client.getConfig();
console.log(config);

// Replace the entire config.
await client.setConfig({ ...config, max_tokens: 4096 });

// Or merge-patch specific fields.
const updated = await client.patchConfig({ max_tokens: 8192 });
console.log(updated.max_tokens);  // 8192

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
  structured_output_retries: 3,
});

// Parsed structured output (matches the schema).
console.log(result.structured_output);
// { capitals: ["Paris", "Berlin", "Madrid"] }

// Schema warnings from provider-specific validation issues.
if (result.schema_warnings) {
  for (const w of result.schema_warnings) {
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
  model: "claude-sonnet-4-5",
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
