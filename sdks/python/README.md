# Meerkat Python SDK

Python SDK for the [Meerkat](https://github.com/lukacf/raik) agent runtime. Communicates with the `rkat` JSON-RPC server over stdio to manage sessions, run agent turns, stream events, and query capabilities.

- **Contract version:** `0.2.0`
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

The `rkat` binary must be installed and available on your `PATH`. The SDK spawns `rkat rpc` as a subprocess and communicates via JSON-RPC 2.0 over newline-delimited JSON on stdin/stdout.

Build from source:

```bash
cargo build -p meerkat-cli --release
# Binary is at target/release/rkat
```

If the binary is not on `PATH`, you can pass its location explicitly:

```python
client = MeerkatClient(rkat_path="/path/to/rkat")
```

If the binary is not found, `MeerkatError` is raised immediately with code `"BINARY_NOT_FOUND"`.

## Quick Start

```python
import asyncio
from meerkat import MeerkatClient

async def main():
    # Connect to rkat rpc
    client = MeerkatClient()
    await client.connect()

    # Create a session and run the first turn
    result = await client.create_session("What is the capital of France?")
    print(result.text)           # "The capital of France is Paris."
    print(result.session_id)     # UUID of the created session

    # Multi-turn: continue the conversation
    result2 = await client.start_turn(result.session_id, "And of Germany?")
    print(result2.text)

    # Clean up
    await client.archive_session(result.session_id)
    await client.close()

asyncio.run(main())
```

## API Reference

### MeerkatClient

```python
from meerkat import MeerkatClient
```

The primary class. Spawns `rkat rpc` as a child process and speaks JSON-RPC over its stdin/stdout.

#### Constructor

```python
MeerkatClient(rkat_path: str = "rkat")
```

| Parameter   | Type  | Default  | Description |
|-------------|-------|----------|-------------|
| `rkat_path` | `str` | `"rkat"` | Path to the `rkat` binary. Resolved via `shutil.which()`. |

Raises `MeerkatError` (code `"BINARY_NOT_FOUND"`) if the binary is not found.

#### `connect()`

```python
async def connect(self) -> MeerkatClient
```

Starts the `rkat rpc` subprocess and performs the initialization handshake:

1. Sends `initialize` and checks the server's `contract_version` against the SDK's `CONTRACT_VERSION` (`"0.2.0"`). Raises `MeerkatError` (code `"VERSION_MISMATCH"`) on incompatibility.
2. Fetches capabilities via `capabilities/get` and caches them internally.

Returns `self` for chaining.

#### `close()`

```python
async def close(self) -> None
```

Terminates the `rkat rpc` subprocess. Sends `SIGTERM` and waits up to 5 seconds; falls back to `SIGKILL` on timeout.

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
    enable_subagents: bool = False,
    enable_memory: bool = False,
    host_mode: bool = False,
    comms_name: Optional[str] = None,
    provider_params: Optional[dict] = None,
) -> WireRunResult
```

Creates a new session and runs the first turn. Maps to the `session/create` JSON-RPC method.

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
| `enable_subagents`           | `bool`          | `False` | Enable sub-agent tools (fork, spawn). |
| `enable_memory`              | `bool`          | `False` | Enable semantic memory (`memory_search` tool). |
| `host_mode`                  | `bool`          | `False` | Run in host mode for inter-agent comms. |
| `comms_name`                 | `Optional[str]` | `None`  | Agent name for comms (required when `host_mode` is `True`). |
| `provider_params`            | `Optional[dict]`| `None`  | Provider-specific parameters (e.g. thinking config). |

Returns a `WireRunResult`.

#### `start_turn()`

```python
async def start_turn(
    self,
    session_id: str,
    prompt: str,
) -> WireRunResult
```

Starts a new turn on an existing session. Maps to the `turn/start` JSON-RPC method.

| Parameter    | Type  | Description |
|-------------|-------|-------------|
| `session_id` | `str` | Session UUID from a prior `create_session()`. |
| `prompt`     | `str` | User prompt for this turn. |

Returns a `WireRunResult`.

#### `interrupt()`

```python
async def interrupt(self, session_id: str) -> None
```

Interrupts a running turn. Maps to `turn/interrupt`.

#### `list_sessions()`

```python
async def list_sessions(self) -> list
```

Lists active sessions. Maps to `session/list`. Returns a list of dicts, each with `session_id` and `state` keys.

#### `read_session()`

```python
async def read_session(self, session_id: str) -> dict
```

Reads session state. Maps to `session/read`. Returns a dict with `session_id` and `state`.

#### `archive_session()`

```python
async def archive_session(self, session_id: str) -> None
```

Archives (removes) a session. Maps to `session/archive`.

#### `get_capabilities()`

```python
async def get_capabilities(self) -> CapabilitiesResponse
```

Returns the cached `CapabilitiesResponse` from the initial handshake. If capabilities were not fetched during `connect()`, re-fetches via `capabilities/get`.

#### `has_capability()`

```python
def has_capability(self, capability_id: str) -> bool
```

Returns `True` if the given capability is available (status is `"Available"`). Checks the cached capabilities; returns `False` if capabilities have not been fetched.

Known capability IDs (from the Rust `CapabilityId` enum):
`sessions`, `streaming`, `structured_output`, `hooks`, `builtins`, `shell`, `comms`, `sub_agents`, `memory_store`, `session_store`, `session_compaction`, `skills`.

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

### WireRunResult

```python
from meerkat import WireRunResult
```

Dataclass returned by `create_session()` and `start_turn()`.

| Field               | Type               | Default | Description |
|---------------------|--------------------|---------|-------------|
| `session_id`        | `str`              | `""`    | Session UUID. |
| `text`              | `str`              | `""`    | Agent response text. |
| `turns`             | `int`              | `0`     | Number of turns completed. |
| `tool_calls`        | `int`              | `0`     | Number of tool calls made. |
| `usage`             | `Optional[WireUsage]` | `None` | Token usage breakdown. |
| `structured_output` | `Optional[Any]`    | `None`  | Parsed structured output (when `output_schema` was provided). |
| `schema_warnings`   | `Optional[list]`   | `None`  | Schema validation warnings, if any. |

### WireUsage

```python
from meerkat import WireUsage
```

Dataclass for token usage.

| Field                    | Type            | Default | Description |
|--------------------------|-----------------|---------|-------------|
| `input_tokens`           | `int`           | `0`     | Input tokens consumed. |
| `output_tokens`          | `int`           | `0`     | Output tokens generated. |
| `total_tokens`           | `int`           | `0`     | Total tokens (input + output). |
| `cache_creation_tokens`  | `Optional[int]` | `None`  | Tokens used for cache creation (provider-specific). |
| `cache_read_tokens`      | `Optional[int]` | `None`  | Tokens read from cache (provider-specific). |

### WireEvent

```python
from meerkat import WireEvent
```

Dataclass for streaming events.

| Field              | Type            | Default | Description |
|--------------------|-----------------|---------|-------------|
| `session_id`       | `str`           | `""`    | Session this event belongs to. |
| `sequence`         | `int`           | `0`     | Monotonic sequence number. |
| `event`            | `Optional[dict]`| `None`  | The agent event payload. |
| `contract_version` | `str`           | `""`    | Contract version of the server. |

### CapabilitiesResponse

```python
from meerkat import CapabilitiesResponse
```

Dataclass returned by `get_capabilities()`.

| Field              | Type                   | Default | Description |
|--------------------|------------------------|---------|-------------|
| `contract_version` | `str`                  | `""`    | Server contract version. |
| `capabilities`     | `list[CapabilityEntry]`| `[]`    | List of capability entries. |

### CapabilityEntry

```python
from meerkat import CapabilityEntry
```

Dataclass for a single capability.

| Field         | Type  | Default       | Description |
|---------------|-------|---------------|-------------|
| `id`          | `str` | `""`          | Capability identifier (e.g. `"sessions"`, `"shell"`). |
| `description` | `str` | `""`          | Human-readable description. |
| `status`      | `str` | `"available"` | Status string: `"Available"`, `"DisabledByPolicy"`, `"NotCompiled"`, or `"NotSupportedByProtocol"`. |

---

## CapabilityChecker

```python
from meerkat import CapabilityChecker
```

Standalone capability-checking helper. Wraps a `CapabilitiesResponse` for convenient querying.

```python
caps = await client.get_capabilities()
checker = CapabilityChecker(caps)

# Check availability
if checker.has("shell"):
    print("Shell tool is available")

# Guard a code path
checker.require("comms")  # raises CapabilityUnavailableError if unavailable

# List all available capabilities
print(checker.available)  # e.g. ["sessions", "streaming", "builtins"]
```

### Methods

| Method     | Signature                           | Description |
|------------|-------------------------------------|-------------|
| `has`      | `has(capability_id: str) -> bool`   | Returns `True` if the capability status is `"Available"`. |
| `require`  | `require(capability_id: str) -> None` | Raises `CapabilityUnavailableError` if not available. |
| `available`| `@property -> list[str]`            | List of all capability IDs with status `"Available"`. |

---

## SkillHelper

```python
from meerkat import SkillHelper
```

Helpers for invoking Meerkat skills. Skills are loaded by the agent from filesystem and embedded sources. To invoke a skill, include its reference (e.g. `/shell-patterns`) in the user prompt. The `SkillHelper` handles this automatically.

```python
helper = SkillHelper(client)

# Check skill support
if helper.is_available():
    # Invoke within an existing session
    result = await helper.invoke(
        session_id, "/shell-patterns", "How do I run a background job?"
    )
    print(result.text)

    # Or create a new session with a skill invocation
    result = await helper.invoke_new_session(
        "/code-review", "Review this function", model="claude-sonnet-4-5"
    )
```

### Constructor

```python
SkillHelper(client)
```

| Parameter | Type            | Description |
|-----------|-----------------|-------------|
| `client`  | `MeerkatClient` | A connected `MeerkatClient` instance. |

### Methods

| Method               | Signature | Description |
|----------------------|-----------|-------------|
| `is_available`       | `is_available() -> bool` | Returns `True` if the `"skills"` capability is available. |
| `require_skills`     | `require_skills() -> None` | Raises `CapabilityUnavailableError` if skills are not available. |
| `invoke`             | `async invoke(session_id: str, skill_reference: str, prompt: str) -> WireRunResult` | Invokes a skill within an existing session by prepending the skill reference to the prompt. |
| `invoke_new_session` | `async invoke_new_session(skill_reference: str, prompt: str, model: Optional[str] = None) -> WireRunResult` | Creates a new session and invokes a skill in the first turn. |

---

## EventStream

```python
from meerkat import EventStream
```

Async iterator that yields `WireEvent` objects from the `rkat rpc` notification stream. Reads from an `asyncio.StreamReader` (the stdout of the rkat process), skips JSON-RPC response messages (which have an `id` field), and only yields notification payloads.

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
| `stream`  | `asyncio.StreamReader`  | The stdout stream from the `rkat rpc` process. |

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
| `"BINARY_NOT_FOUND"`    | Client    | `rkat` binary not found on PATH. |
| `"NOT_CONNECTED"`       | Client    | Operation attempted before `connect()`. |
| `"CONNECTION_CLOSED"`   | Client    | The `rkat rpc` process closed unexpectedly. |
| `"VERSION_MISMATCH"`    | Client    | Server contract version incompatible with SDK. |
| `"CAPABILITY_UNAVAILABLE"` | Client | Capability check failed via `require_capability()` or `SkillHelper.require_skills()`. |

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
    result = await client.start_turn("nonexistent-session-id", "hello")
except MeerkatError as e:
    print(f"Error code: {e.code}")
    print(f"Message: {e.message}")
    if e.details:
        print(f"Details: {e.details}")
```

---

## Version Compatibility

The SDK negotiates version compatibility during `connect()`. The contract version follows semver conventions:

- **0.x:** The SDK requires an exact minor version match (e.g., SDK `0.2.0` is compatible with server `0.2.x` but not `0.3.0`).
- **1.0+:** The SDK requires the same major version (e.g., SDK `1.0.0` is compatible with server `1.x.x` but not `2.0.0`).

The current contract version is `0.2.0`, defined as the `CONTRACT_VERSION` constant:

```python
from meerkat import CONTRACT_VERSION
print(CONTRACT_VERSION)  # "0.2.0"
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
    result = await client.create_session(
        "You are a helpful math tutor. What is 12 * 15?",
        model="claude-sonnet-4-5",
    )
    print(f"Turn 1: {result.text}")
    print(f"Tokens used: {result.usage.total_tokens}")

    # Follow-up turn in the same session
    result2 = await client.start_turn(
        result.session_id,
        "Now divide that result by 3.",
    )
    print(f"Turn 2: {result2.text}")

    # Clean up
    await client.archive_session(result.session_id)
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

    result = await client.create_session(
        "Give me information about Tokyo.",
        output_schema=schema,
        structured_output_retries=3,
    )
    print(f"Text: {result.text}")
    print(f"Structured: {result.structured_output}")
    if result.schema_warnings:
        print(f"Warnings: {result.schema_warnings}")

    await client.archive_session(result.session_id)
    await client.close()

asyncio.run(structured())
```

### Capability Checking

```python
import asyncio
from meerkat import MeerkatClient, CapabilityChecker, CapabilityUnavailableError

async def check_caps():
    client = MeerkatClient()
    await client.connect()

    # Via MeerkatClient directly
    if client.has_capability("shell"):
        result = await client.create_session(
            "List files in /tmp",
            enable_builtins=True,
            enable_shell=True,
        )

    # Via CapabilityChecker
    caps = await client.get_capabilities()
    checker = CapabilityChecker(caps)

    print(f"Available: {checker.available}")

    try:
        checker.require("comms")
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
        print(f"  {s['session_id']}: {s['state']}")

    # Read a specific session
    info = await client.read_session(s1.session_id)
    print(f"Session {info['session_id']} state: {info['state']}")

    # Archive sessions
    await client.archive_session(s1.session_id)
    await client.archive_session(s2.session_id)

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

    result = await client.create_session(
        "Create a file called hello.txt with 'Hello World' in it",
        enable_builtins=True,
        enable_shell=True,
    )
    print(f"Response: {result.text}")
    print(f"Tool calls made: {result.tool_calls}")

    await client.archive_session(result.session_id)
    await client.close()

asyncio.run(with_tools())
```

## Generated Types

The `meerkat.generated` subpackage contains wire types and error classes generated from the Meerkat contract definitions. These are re-exported from the top-level `meerkat` package for convenience. The generated types include:

- `meerkat.generated.types`: `CONTRACT_VERSION`, `WireUsage`, `WireRunResult`, `WireEvent`, `CapabilityEntry`, `CapabilitiesResponse`, `CommsParams`, `SkillsParams`
- `meerkat.generated.errors`: `MeerkatError`, `CapabilityUnavailableError`, `SessionNotFoundError`, `SkillNotFoundError`

The top-level `meerkat.errors` module defines identical error classes that are used by the SDK itself. The `meerkat.types` module re-exports all types from `meerkat.generated.types`.

## Running Tests

```bash
# Install dev dependencies
pip install -e "sdks/python[dev]"

# Run type conformance tests (no rkat binary needed)
pytest sdks/python/tests/test_types.py -v

# Run E2E tests (requires rkat on PATH)
pytest sdks/python/tests/test_e2e.py -v

# Run E2E smoke tests (requires rkat on PATH + ANTHROPIC_API_KEY)
pytest sdks/python/tests/test_e2e_smoke.py -v
```
