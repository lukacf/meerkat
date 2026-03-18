# Meerkat Python SDK

Python SDK for the [Meerkat](https://github.com/lukacf/meerkat) runtime. The SDK is a thin session-first wrapper over the same runtime-backed contracts used by the CLI, REST, JSON-RPC, and MCP surfaces. It spawns the `rkat-rpc` server over stdio, then exposes those settled semantics as Python objects.

- **Contract version:** `0.4.11`
- **Python:** `>=3.10`
- **License:** MIT OR Apache-2.0

## Installation

From the SDK directory:

```bash
pip install sdks/python
```

Or in editable/development mode:

```bash
pip install -e "sdks/python[dev]"
```

The package is named `meerkat-sdk` (defined in `pyproject.toml`). It has zero runtime dependencies -- only `pytest` and `pytest-asyncio` are needed for development.

## Prerequisites

The `rkat-rpc` binary must be installed and available on your `PATH`. It communicates via JSON-RPC 2.0 over newline-delimited JSON on stdin/stdout.

Build from source:

```bash
cargo build -p meerkat-rpc --release
# Binary is at target/release/rkat-rpc
```

If the binary is not on `PATH`, you can pass its location explicitly:

```python
client = MeerkatClient(rkat_path="/path/to/rkat-rpc")
```

If the binary is not found, `MeerkatError` is raised immediately with code `"BINARY_NOT_FOUND"`.

## Quick Start

```python
import asyncio
from meerkat import MeerkatClient

async def main():
    async with MeerkatClient() as client:
        # Create a runtime-backed session and run the first turn immediately.
        session = await client.create_session("What is the capital of France?")
        print(session.text)
        print(session.id)

        # Multi-turn: continue through the same session handle.
        result = await session.turn("And of Germany?")
        print(result.text)

        # Read canonical transcript history if needed.
        history = await session.history(limit=10)
        print(history.message_count)

        await session.archive()

asyncio.run(main())
```

## API Reference

### MeerkatClient

```python
from meerkat import MeerkatClient
```

The primary class. Spawns `rkat-rpc` as a child process and speaks JSON-RPC over its stdin/stdout. It owns the subprocess lifecycle; `Session` and `DeferredSession` are the runtime-backed handles returned from session creation.

#### Constructor

```python
MeerkatClient(rkat_path: str = "rkat-rpc")
```

| Parameter   | Type  | Default  | Description |
|-------------|-------|----------|-------------|
| `rkat_path` | `str` | `"rkat-rpc"` | Path to the RPC binary. |

Raises `MeerkatError` (code `"BINARY_NOT_FOUND"`) if the binary is not found.

#### `connect()`

```python
async def connect(self) -> MeerkatClient
```

Starts the `rkat-rpc` subprocess and performs the initialization handshake:

1. Sends `initialize` and checks the server's `contract_version` against the SDK's `CONTRACT_VERSION` (`"0.4.11"`). Raises `MeerkatError` (code `"VERSION_MISMATCH"`) on incompatibility.
2. Fetches capabilities via `capabilities/get` and caches them internally.

Returns `self` for chaining.

#### `close()`

```python
async def close(self) -> None
```

Terminates the `rkat-rpc` subprocess. Sends `SIGTERM` and waits up to 5 seconds; falls back to `SIGKILL` on timeout.

#### `create_session()`

```python
async def create_session(
    self,
    prompt: str,
    model: Optional[str] = None,
    provider: Optional[str] = None,
    system_prompt: Optional[str] = None,
    max_tokens: Optional[int] = None,
    output_schema: Optional[dict] = None,
    structured_output_retries: int = 2,
    hooks_override: Optional[dict] = None,
    enable_builtins: bool = False,
    enable_shell: bool = False,
    enable_memory: bool = False,
    enable_mob: bool = False,
    host_mode: bool = False,
    comms_name: Optional[str] = None,
    peer_meta: Optional[dict] = None,
    budget_limits: Optional[dict] = None,
    provider_params: Optional[dict] = None,
    preload_skills: Optional[list[str]] = None,
    skill_refs: Optional[list[SkillRef]] = None,
    skill_references: Optional[list[str]] = None,
) -> Session
```

Creates a new session, runs the first turn, and returns a runtime-backed `Session` wrapper. The wrapper caches the most recent `RunResult` and exposes `session.turn()`, `session.stream()`, `session.history()`, `session.archive()`, `session.invoke_skill()`, and comms helpers without adding wrapper-local execution behavior.

| Parameter                    | Type            | Default | Description |
|------------------------------|-----------------|---------|-------------|
| `prompt`                     | `str`           | --      | Initial user prompt (required). |
| `model`                      | `Optional[str]` | `None`  | Model name (e.g. `"claude-sonnet-4-5"`, `"gpt-5.2"`). |
| `provider`                   | `Optional[str]` | `None`  | Provider name: `"anthropic"`, `"openai"`, or `"gemini"`. |
| `system_prompt`              | `Optional[str]` | `None`  | Custom system prompt override. |
| `max_tokens`                 | `Optional[int]` | `None`  | Max tokens per turn. |
| `output_schema`              | `Optional[dict]`| `None`  | JSON schema for structured output extraction. |
| `structured_output_retries`  | `int`           | `2`     | Retries for structured output validation. |
| `hooks_override`             | `Optional[dict]`| `None`  | Run-scoped hook overrides. |
| `enable_builtins`            | `bool`          | `False` | Enable built-in tools (task management, etc.). |
| `enable_shell`               | `bool`          | `False` | Enable shell tool (requires `enable_builtins`). |
| `enable_memory`              | `bool`          | `False` | Enable semantic memory (`memory_search` tool). |
| `enable_mob`                 | `bool`          | `False` | Enable mob orchestration helpers. |
| `host_mode`                  | `bool`          | `False` | Run in host mode for inter-agent comms. |
| `comms_name`                 | `Optional[str]` | `None`  | Agent name for comms (required when `host_mode` is `True`). |
| `peer_meta`                  | `Optional[dict]`| `None`  | Metadata advertised to peer comms surfaces. |
| `budget_limits`              | `Optional[dict]`| `None`  | Runtime budget limits for the session. |
| `provider_params`            | `Optional[dict]`| `None`  | Provider-specific parameters (e.g. thinking config). |
| `preload_skills`             | `Optional[list[str]]` | `None` | Skill source UUIDs to load before the run. |
| `skill_refs`                 | `Optional[list[SkillRef]]` | `None` | Structured skill references in canonical `{source_uuid, skill_name}` form. |
| `skill_references`           | `Optional[list[str]]` | `None` | Legacy skill-reference strings; prefer `skill_refs`. |

Returns a `Session`.

#### `create_deferred_session()`

```python
async def create_deferred_session(
    self,
    prompt: str | list[dict],
    *,
    ...
) -> DeferredSession
```

Creates a session without executing the first turn. Use `await deferred.start_turn(...)` when you want to set up the session now and begin the run later through the canonical runtime path.

#### `read_session_history()`

```python
async def read_session_history(
    self,
    session_id: str,
    *,
    offset: int = 0,
    limit: int | None = None,
) -> SessionHistory
```

Reads the committed transcript page for a session. `Session.history()` is the convenience wrapper for the same runtime-backed call.

#### `list_sessions()`

```python
async def list_sessions(self) -> list
```

Lists active sessions. Maps to `session/list`. Returns `list[SessionInfo]`.

#### `read_session()`

```python
async def read_session(self, session_id: str) -> dict
```

Reads session state. Maps to `session/read`. Returns a dict with `session_id`, session metadata, and state payload.

#### `capabilities`

```python
client.capabilities
```

Returns the cached `list[Capability]` from the initial handshake.

#### `has_capability()`

```python
def has_capability(self, capability_id: str) -> bool
```

Returns `True` if the given capability is available (status is `"Available"`). Checks the cached capabilities; returns `False` if capabilities have not been fetched.

#### Session lifecycle on wrappers

Archive, interrupt, history, and event-subscription operations live on the runtime-backed session wrappers:

```python
await session.interrupt()
await session.archive()
history = await session.history(limit=20)
subscription = await session.subscribe_events()
```

Known capability IDs include `sessions`, `streaming`, `structured_output`, `hooks`, `builtins`, `shell`, `comms`, `memory_store`, `session_store`, `session_compaction`, `skills`, and other optional runtime modules compiled into the backend.

#### `require_capability()`

```python
def require_capability(self, capability_id: str) -> None
```

Raises `CapabilityUnavailableError` if the capability is not available.

#### `get_config()`

```python
async def get_config(self) -> dict
```

Returns the runtime configuration. Maps to `config/get`.

#### `set_config()`

```python
async def set_config(self, config: dict) -> None
```

Replaces the runtime configuration. Maps to `config/set`. The `config` dict is sent as `{"config": config}`.

#### `patch_config()`

```python
async def patch_config(self, patch: dict) -> dict
```

Merge-patches the runtime configuration. Maps to `config/patch`. The `patch` dict is sent as `{"patch": patch}`. Returns the updated config.

---

## Public types

The Python SDK exposes Python-native domain types from the `meerkat` package. The JSON-RPC wire dataclasses remain internal.

- `RunResult` is available through `session.last_result`, `await session.turn(...)`, `await deferred.start_turn(...)`, and `events.result`.
- `Usage`, `SessionInfo`, `Capability`, and `SchemaWarning` mirror the runtime response shapes directly.
- `Session` and `DeferredSession` are the canonical runtime-backed wrappers for session lifecycle.
- `EventStream` yields typed event dataclasses such as `TextDelta`, `TurnCompleted`, and `ToolExecutionCompleted`.

Use the client and session helpers directly for capability and skill flows:

```python
async with MeerkatClient() as client:
    if client.has_capability("skills"):
        session = await client.create_session("Review this function")
        result = await session.invoke_skill(
            SkillKey(source_uuid="source-123", skill_name="code-review"),
            "Focus on performance regressions.",
        )
        print(result.text)
```

---

## EventStream

```python
from meerkat import EventStream
```

Async iterator that yields `WireEvent` objects from the `rkat-rpc` notification stream. Reads from an `asyncio.StreamReader` (the stdout of the rkat process), skips JSON-RPC response messages (which have an `id` field), and only yields notification payloads.

```python
# Low-level usage with the process stdout
stream = EventStream(client._process.stdout)
async for event in stream:
    print(f"[{event.session_id}] seq={event.sequence}: {event.event}")
```

### Constructor

```python
EventStream(stream: asyncio.StreamReader)
```

| Parameter | Type                    | Description |
|-----------|-------------------------|-------------|
| `stream`  | `asyncio.StreamReader`  | The stdout stream from the `rkat-rpc` process. |

### Async Iterator Protocol

`EventStream` implements `__aiter__` and `__anext__`. It yields `WireEvent` instances until the stream is closed (raises `StopAsyncIteration`).

---

## Config Management

The SDK exposes the full config lifecycle via three methods:

```python
# Read current configuration
config = await client.get_config()
print(config["agent"])

# Replace entire configuration
config["max_tokens"] = 4096
await client.set_config(config)

# Merge-patch specific fields (returns updated config)
updated = await client.patch_config({"max_tokens": 2048})
print(updated["max_tokens"])  # 2048
```

---

## Error Handling

### Error Classes

All errors inherit from `MeerkatError`.

```python
from meerkat import (
    MeerkatError,
    CapabilityUnavailableError,
    SessionNotFoundError,
    SkillNotFoundError,
)
```

| Class                        | Description |
|------------------------------|-------------|
| `MeerkatError`               | Base error. Attributes: `code` (str), `message` (str), `details` (any), `capability_hint` (any). |
| `CapabilityUnavailableError` | Raised when a required capability is not available in the runtime. |
| `SessionNotFoundError`       | Raised when a session is not found. |
| `SkillNotFoundError`         | Raised when a skill reference cannot be resolved. |

### Error Codes

The SDK surfaces JSON-RPC error codes as the string `code` attribute on `MeerkatError`. When the server returns an error, the numeric JSON-RPC code is converted to a string.

SDK-specific codes raised by the client itself:

| Code                    | Origin    | Description |
|-------------------------|-----------|-------------|
| `"BINARY_NOT_FOUND"`    | Client    | `rkat-rpc` binary not found on PATH. |
| `"NOT_CONNECTED"`       | Client    | Operation attempted before `connect()`. |
| `"CONNECTION_CLOSED"`   | Client    | The `rkat-rpc` process closed unexpectedly. |
| `"VERSION_MISMATCH"`    | Client    | Server contract version incompatible with SDK. |
| `"CAPABILITY_UNAVAILABLE"` | Client | Capability check failed via `require_capability()`. |

Server-side error codes (from `meerkat-contracts`):

| Code (wire)                | JSON-RPC Code | Description |
|----------------------------|---------------|-------------|
| `SESSION_NOT_FOUND`        | `-32001`      | Session does not exist. |
| `SESSION_BUSY`             | `-32002`      | Session is currently running a turn. |
| `SESSION_NOT_RUNNING`      | `-32003`      | No turn in progress to interrupt. |
| `PROVIDER_ERROR`           | `-32010`      | LLM provider returned an error. |
| `BUDGET_EXHAUSTED`         | `-32011`      | Token or turn budget exceeded. |
| `HOOK_DENIED`              | `-32012`      | A hook blocked the operation. |
| `AGENT_ERROR`              | `-32013`      | Internal agent execution error. |
| `CAPABILITY_UNAVAILABLE`   | `-32020`      | Required capability not compiled or disabled. |
| `SKILL_NOT_FOUND`          | `-32021`      | Skill reference could not be resolved. |
| `SKILL_RESOLUTION_FAILED`  | `-32022`      | Skill resolution process failed. |
| `INVALID_PARAMS`           | `-32602`      | Invalid method parameters. |
| `INTERNAL_ERROR`           | `-32603`      | Internal server error. |

### Error Handling Example

```python
from meerkat import MeerkatClient, MeerkatError, CapabilityUnavailableError

client = MeerkatClient()
await client.connect()

try:
    result = await client.read_session("nonexistent-session-id")
except MeerkatError as e:
    print(f"Error code: {e.code}")
    print(f"Message: {e.message}")
    if e.details:
        print(f"Details: {e.details}")
```

---

## Version Compatibility

The SDK negotiates version compatibility during `connect()`. The contract version follows semver conventions:

- **0.x:** The SDK requires an exact minor version match (e.g., SDK `0.4.11` is compatible with server `0.4.x` but not `0.5.0`).
- **1.0+:** The SDK requires the same major version (e.g., SDK `1.0.0` is compatible with server `1.x.x` but not `2.0.0`).

The current contract version is `0.4.11`, defined as the `CONTRACT_VERSION` constant:

```python
from meerkat import CONTRACT_VERSION
print(CONTRACT_VERSION)  # "0.4.11"
```

---

## Examples

### Multi-Turn Conversation

```python
import asyncio
from meerkat import MeerkatClient

async def multi_turn():
    client = MeerkatClient()
    await client.connect()

    # First turn
    session = await client.create_session(
        "You are a helpful math tutor. What is 12 * 15?",
        model="claude-sonnet-4-5",
    )
    print(f"Turn 1: {session.text}")
    print(f"Tokens used: {session.usage.total_tokens}")

    # Follow-up turn in the same session
    result2 = await session.turn("Now divide that result by 3.")
    print(f"Turn 2: {result2.text}")

    # Clean up
    await session.archive()
    await client.close()

asyncio.run(multi_turn())
```

### Structured Output

```python
import asyncio
from meerkat import MeerkatClient

async def structured():
    client = MeerkatClient()
    await client.connect()

    schema = {
        "type": "object",
        "properties": {
            "city": {"type": "string"},
            "country": {"type": "string"},
            "population": {"type": "integer"},
        },
        "required": ["city", "country", "population"],
    }

    session = await client.create_session(
        "Give me information about Tokyo.",
        output_schema=schema,
        structured_output_retries=3,
    )
    print(f"Text: {session.text}")
    print(f"Structured: {session.structured_output}")
    if session.last_result.schema_warnings:
        print(f"Warnings: {session.last_result.schema_warnings}")

    await session.archive()
    await client.close()

asyncio.run(structured())
```

### Capability Checking

```python
import asyncio
from meerkat import MeerkatClient, CapabilityUnavailableError

async def check_caps():
    client = MeerkatClient()
    await client.connect()

    # Capability checks live on MeerkatClient directly.
    if client.has_capability("shell"):
        session = await client.create_session(
            "List files in /tmp",
            enable_builtins=True,
            enable_shell=True,
        )
        print(session.text)

    try:
        client.require_capability("comms")
        # comms is available, proceed
    except CapabilityUnavailableError:
        print("Comms capability is not available")

    await client.close()

asyncio.run(check_caps())
```

### Session Management

```python
import asyncio
from meerkat import MeerkatClient

async def manage_sessions():
    client = MeerkatClient()
    await client.connect()

    # Create multiple sessions
    s1 = await client.create_session("Session 1 prompt")
    s2 = await client.create_session("Session 2 prompt")

    # List all active sessions
    sessions = await client.list_sessions()
    for s in sessions:
        print(f"  {s.session_id}: {s.message_count} messages")

    # Read a specific session
    info = await client.read_session(s1.id)
    print(f"Session {info['session_id']} state: {info['state']}")

    # Archive sessions
    await s1.archive()
    await s2.archive()

    await client.close()

asyncio.run(manage_sessions())
```

### Using with Tools Enabled

```python
import asyncio
from meerkat import MeerkatClient

async def with_tools():
    client = MeerkatClient()
    await client.connect()

    session = await client.create_session(
        "Create a file called hello.txt with 'Hello World' in it",
        enable_builtins=True,
        enable_shell=True,
    )
    print(f"Response: {session.text}")
    print(f"Tool calls made: {session.tool_calls}")

    await session.archive()
    await client.close()

asyncio.run(with_tools())
```

## Generated Types

The `meerkat.generated` subpackage still exists for generated contract material such as `CONTRACT_VERSION`, MCP parameter types, and internal wire compatibility helpers. The public SDK surface re-exports Python-native domain types from `meerkat.types` and session-first wrappers from the top-level `meerkat` package.

## Running Tests

```bash
# Install dev dependencies
pip install -e "sdks/python[dev]"

# Run type conformance tests (no rkat-rpc binary needed)
pytest sdks/python/tests/test_types.py -v

# Run E2E tests (requires rkat-rpc on PATH)
pytest sdks/python/tests/test_e2e.py -v

# Run E2E smoke tests (requires rkat-rpc on PATH + ANTHROPIC_API_KEY)
pytest sdks/python/tests/test_e2e_smoke.py -v
```
