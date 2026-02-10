# Meerkat TypeScript SDK

TypeScript client for the [Meerkat](https://github.com/lukacf/raik) agent runtime. Communicates with a local `rkat rpc` subprocess over JSON-RPC 2.0 (newline-delimited JSON on stdin/stdout).

## Installation

```bash
npm install @meerkat/sdk
```

## Prerequisites

- **`rkat` binary on PATH** -- build it from the Meerkat repo with `cargo build -p meerkat-cli`, then ensure the resulting `rkat` binary is in your `$PATH`.
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
import { MeerkatClient } from "@meerkat/sdk";

const client = new MeerkatClient();

// Connect spawns `rkat rpc`, performs the initialize handshake,
// and fetches runtime capabilities.
await client.connect();

// Create a session (runs the first turn immediately).
const result = await client.createSession({
  prompt: "What is the capital of Sweden?",
});

console.log(result.text);         // "Stockholm..."
console.log(result.session_id);   // UUID of the new session
console.log(result.usage);        // { input_tokens, output_tokens, total_tokens, ... }

// Multi-turn: send a follow-up in the same session.
const followUp = await client.startTurn(
  result.session_id,
  "And what is its population?",
);
console.log(followUp.text);

// Clean up.
await client.archiveSession(result.session_id);
await client.close();
```

## API Reference: MeerkatClient

### Constructor

```ts
new MeerkatClient(rkatPath?: string)
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `rkatPath` | `string` | `"rkat"` | Path to the `rkat` binary. Override if it is not on your `$PATH`. |

### connect()

```ts
async connect(): Promise<this>
```

Spawns `rkat rpc` as a child process, performs the `initialize` handshake, checks contract version compatibility, and fetches runtime capabilities via `capabilities/get`. Returns `this` for chaining.

Throws `MeerkatError` with code `"VERSION_MISMATCH"` if the server's contract version is incompatible with the SDK's `CONTRACT_VERSION`.

### close()

```ts
async close(): Promise<void>
```

Kills the `rkat rpc` subprocess and cleans up resources.

### createSession(params)

```ts
async createSession(params: {
  prompt: string;
  model?: string;
  provider?: string;
  system_prompt?: string;
  max_tokens?: number;
  output_schema?: Record<string, unknown>;
  structured_output_retries?: number;
  hooks_override?: Record<string, unknown>;
  enable_builtins?: boolean;
  enable_shell?: boolean;
  enable_subagents?: boolean;
  enable_memory?: boolean;
  host_mode?: boolean;
  comms_name?: string;
  provider_params?: Record<string, unknown>;
}): Promise<WireRunResult>
```

Creates a new session and immediately runs the first turn with the given prompt. Returns a `WireRunResult`.

**Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `prompt` | `string` | **(required)** | The user prompt for the first turn. |
| `model` | `string` | Server default (typically `claude-sonnet-4-5`) | LLM model name (e.g. `"gpt-5.2"`, `"gemini-3-flash-preview"`, `"claude-opus-4-6"`). |
| `provider` | `string` | Auto-detected from model | Force a specific provider (`"anthropic"`, `"openai"`, `"gemini"`). |
| `system_prompt` | `string` | `undefined` | Override the default system prompt. |
| `max_tokens` | `number` | `undefined` | Maximum output tokens for the LLM response. |
| `output_schema` | `Record<string, unknown>` | `undefined` | JSON Schema for structured output extraction. |
| `structured_output_retries` | `number` | `2` (server default) | Max retries for structured output validation. |
| `hooks_override` | `Record<string, unknown>` | `undefined` | Run-scoped hook overrides. |
| `enable_builtins` | `boolean` | `false` | Enable built-in tools (task management, etc.). |
| `enable_shell` | `boolean` | `false` | Enable the shell tool (requires `enable_builtins`). |
| `enable_subagents` | `boolean` | `false` | Enable sub-agent tools (fork, spawn). |
| `enable_memory` | `boolean` | `false` | Enable semantic memory (memory_search tool + compaction indexing). |
| `host_mode` | `boolean` | `false` | Run in host mode for inter-agent comms. |
| `comms_name` | `string` | `undefined` | Agent name for comms (required when `host_mode` is `true`). |
| `provider_params` | `Record<string, unknown>` | `undefined` | Provider-specific parameters (e.g. thinking config). |

### startTurn(sessionId, prompt)

```ts
async startTurn(sessionId: string, prompt: string): Promise<WireRunResult>
```

Starts a new turn on an existing session. Returns a `WireRunResult`.

| Parameter | Type | Description |
|-----------|------|-------------|
| `sessionId` | `string` | UUID of an existing session. |
| `prompt` | `string` | The user prompt for this turn. |

### interrupt(sessionId)

```ts
async interrupt(sessionId: string): Promise<void>
```

Cancels an in-flight turn on the specified session. No-op if the session is idle.

### listSessions()

```ts
async listSessions(): Promise<unknown[]>
```

Returns an array of session info objects (each containing `session_id` and `state`).

### readSession(sessionId)

```ts
async readSession(sessionId: string): Promise<Record<string, unknown>>
```

Returns the current state of a session (contains `session_id` and `state`).

### archiveSession(sessionId)

```ts
async archiveSession(sessionId: string): Promise<void>
```

Removes a session from the runtime. The session will no longer appear in `listSessions()`.

### getCapabilities()

```ts
async getCapabilities(): Promise<CapabilitiesResponse>
```

Returns the cached capabilities response. If capabilities were not yet fetched (e.g. `connect()` was not called), fetches them from the server.

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
| `"sub_agents"` | Sub-agent tools (fork, spawn) |
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

## Wire Types

### WireRunResult

Returned by `createSession()` and `startTurn()`.

```ts
interface WireRunResult {
  session_id: string;
  text: string;
  turns: number;
  tool_calls: number;
  usage: WireUsage;
  structured_output?: unknown;
  schema_warnings?: Array<{
    provider: string;
    path: string;
    message: string;
  }>;
}
```

### WireUsage

Token usage statistics.

```ts
interface WireUsage {
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  cache_creation_tokens?: number;
  cache_read_tokens?: number;
}
```

### WireEvent

Event emitted as a JSON-RPC notification during a turn.

```ts
interface WireEvent {
  session_id: string;
  sequence: number;
  event: Record<string, unknown>;
  contract_version: string;
}
```

### CapabilitiesResponse

```ts
interface CapabilitiesResponse {
  contract_version: string;
  capabilities: CapabilityEntry[];
}
```

### CapabilityEntry

```ts
interface CapabilityEntry {
  id: string;
  description: string;
  status: string;  // "Available", "DisabledByPolicy", "NotCompiled", etc.
}
```

## CapabilityChecker

Standalone helper for checking runtime capabilities. Useful when you want to gate entire code paths based on what the server supports.

```ts
import { MeerkatClient, CapabilityChecker } from "@meerkat/sdk";

const client = new MeerkatClient();
await client.connect();

const caps = await client.getCapabilities();
const checker = new CapabilityChecker(caps);

// Check if a capability is available.
if (checker.has("comms")) {
  console.log("Comms is available");
}

// Throw if a capability is missing.
checker.require("skills");  // throws CapabilityUnavailableError if unavailable

// List all available capability IDs.
console.log(checker.available);  // ["sessions", "streaming", ...]
```

### Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `has` | `has(capabilityId: string): boolean` | Returns `true` if the capability status is `"Available"`. |
| `require` | `require(capabilityId: string): void` | Throws `CapabilityUnavailableError` if the capability is not available. |
| `available` | `get available(): string[]` | Getter that returns all capability IDs with status `"Available"`. |

## SkillHelper

Convenience wrapper for invoking Meerkat skills. Skills are loaded by the agent from filesystem and embedded sources. To invoke a skill, include its reference (e.g. `/shell-patterns`) in the user prompt.

```ts
import { MeerkatClient, SkillHelper } from "@meerkat/sdk";

const client = new MeerkatClient();
await client.connect();

const helper = new SkillHelper(client);

// Check if skills are available in this runtime.
if (helper.isAvailable()) {
  // Invoke a skill in an existing session.
  const result = await helper.invoke(
    sessionId,
    "/shell-patterns",
    "How do I run a background job?",
  );
  console.log(result.text);
}

// Or create a new session and invoke a skill in one call.
const result = await helper.invokeNewSession(
  "/code-review",
  "Review this function for performance issues",
  "claude-opus-4-6",  // optional model override
);
```

### Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `isAvailable` | `isAvailable(): boolean` | Returns `true` if the `"skills"` capability is available. |
| `requireSkills` | `requireSkills(): void` | Throws `CapabilityUnavailableError` if skills are not available. |
| `invoke` | `invoke(sessionId: string, skillReference: string, prompt: string): Promise<WireRunResult>` | Invokes a skill in an existing session. Prepends the skill reference to the prompt and calls `startTurn`. |
| `invokeNewSession` | `invokeNewSession(skillReference: string, prompt: string, model?: string): Promise<WireRunResult>` | Creates a new session and invokes a skill in the first turn. |

## EventStream

Async iterator that yields `WireEvent` objects from JSON-RPC notifications emitted by `rkat rpc` during a turn. Filters out response messages (which have an `id` field) and only yields notification payloads.

```ts
import { createInterface } from "node:readline";
import { EventStream } from "@meerkat/sdk";
import type { Interface } from "node:readline";

// The EventStream wraps a readline interface attached to rkat's stdout.
// In practice you would get the readline from the child process:
const rl: Interface = createInterface({ input: process.stdin });
const stream = new EventStream(rl);

for await (const event of stream) {
  console.log(event.session_id, event.sequence, event.event);
}
```

The `EventStream` class implements `AsyncIterable<WireEvent>`. It buffers events internally and resolves waiting consumers as events arrive. When the underlying readline interface closes, the iterator completes.

> **Note:** `MeerkatClient` handles the JSON-RPC response/notification multiplexing internally. `EventStream` is a lower-level primitive for advanced use cases where you manage the `rkat rpc` subprocess yourself.

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
} from "@meerkat/sdk";

const client = new MeerkatClient();

try {
  await client.connect();
  const result = await client.createSession({ prompt: "Hello" });
  console.log(result.text);
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

The SDK exports `CONTRACT_VERSION` (currently `"0.1.0"`). During `connect()`, the SDK checks that the server's contract version is compatible:

- While the major version is `0`, minor versions must match exactly (e.g. SDK `0.1.x` requires server `0.1.x`).
- Once `1.0.0` is reached, major versions must match (standard semver).

```ts
import { CONTRACT_VERSION } from "@meerkat/sdk";
console.log(CONTRACT_VERSION);  // "0.1.0"
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

const result = await client.createSession({
  prompt: "List three European capitals",
  output_schema: {
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
const session = await client.createSession({
  prompt: "My name is Alice.",
  model: "claude-sonnet-4-5",
});

// Follow-up turns reuse the session_id.
const turn2 = await client.startTurn(session.session_id, "What is my name?");
console.log(turn2.text);  // Should mention "Alice"

// Check session state.
const state = await client.readSession(session.session_id);
console.log(state);

// List all active sessions.
const sessions = await client.listSessions();
console.log(`Active sessions: ${sessions.length}`);

// Clean up.
await client.archiveSession(session.session_id);
await client.close();
```

## License

MIT OR Apache-2.0
