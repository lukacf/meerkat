# Future SDKs: Python and Node.js Bindings

This document outlines the design for Python and Node.js/TypeScript SDKs that bind to the Meerkat Rust core. Both SDKs expose the same user-facing functionality as CLI, REST, and MCP interfaces.

## Scope

The SDKs expose the **external user-facing API** only:

- `agent.run(prompt)` / `agent.resume(session_id, prompt)`
- Configuration (model, budget, MCP servers, built-in tools)
- Event streaming for progress
- Session management (list, load, delete)

The SDKs do **not** expose internal extensibility points (custom LLM providers, tool dispatchers, or session stores). Users consume the built-in Rust implementations.

## Requirements Alignment

Per `spec.yaml`, both SDKs must honor the same contracts as CLI/REST/MCP:

| Requirement | Description |
|-------------|-------------|
| REQ-001 | Must use `AgentFactory` for agent construction |
| REQ-004 | Config-only defaults (no hardcoded constants) |
| REQ-018 | Use `ProviderResolver` for model inference and API key lookup |
| REQ-019 | SDK defaults to memory store unless persistence path provided |
| REQ-021 | `ConfigStore` abstraction (File/Memory implementations) |
| REQ-022 | Config paths: `~/.rkat/config.toml` (global), `.rkat/config.toml` (local) |
| REQ-024 | Programmatic config view/edit/override APIs |
| REQ-025 | Env vars only for secrets |
| REQ-026 | No quick builder - must use AgentFactory-based construction |

---

## Python SDK

### Tooling

- **PyO3** - Rust bindings for Python
- **maturin** - Build/publish tool for PyO3 projects
- **pyo3-async-runtimes** - Bridge between tokio and Python asyncio

### Project Structure

```
meerkat-python/
├── Cargo.toml              # pyo3, pyo3-async-runtimes, tokio
├── pyproject.toml          # maturin config
├── src/
│   ├── lib.rs              # #[pymodule] entry point
│   ├── agent.rs            # PyAgent, PyAgentBuilder
│   ├── config.rs           # PyConfig, PyConfigStore
│   ├── result.rs           # PyRunResult, PyUsage
│   ├── events.rs           # PyAgentEvent, streaming
│   └── sessions.rs         # PySessionManager
└── python/
    └── meerkat/
        ├── __init__.py     # Re-exports
        └── _meerkat.pyi    # Type stubs
```

### Cargo.toml

```toml
[package]
name = "meerkat-python"
version = "0.1.0"
edition = "2021"

[lib]
name = "meerkat"
crate-type = ["cdylib"]

[dependencies]
pyo3 = { version = "0.23", features = ["extension-module", "abi3-py39"] }
pyo3-async-runtimes = { version = "0.23", features = ["tokio-runtime"] }
tokio = { version = "1", features = ["full"] }

# Workspace crates
meerkat-core = { path = "../meerkat-core" }
meerkat-client = { path = "../meerkat-client" }
meerkat-store = { path = "../meerkat-store" }
meerkat-tools = { path = "../meerkat-tools" }
meerkat-agent = { path = "../meerkat-agent" }
meerkat-mcp-client = { path = "../meerkat-mcp-client" }
```

### API Design

```python
import meerkat

# === Simple One-Shot Usage ===
result = meerkat.run("What is 2 + 2?")
print(result.text)
print(result.session_id)  # For resuming later

# === Resume Existing Session ===
result = meerkat.resume("session-uuid", "Now multiply by 3")

# === Configuration via Agent Class ===
agent = meerkat.Agent(
    model="claude-sonnet-4-5",
    system_prompt="You are a helpful assistant.",
    max_tokens_per_turn=4096,
    temperature=0.7,
    provider_params={"thinking_budget": 10000},
)

result = agent.run("Analyze this code")
result = agent.resume(session_id, "Continue from here")

# === Budget Configuration ===
agent = meerkat.Agent(
    model="claude-sonnet-4-5",
    budget=meerkat.Budget(
        max_tokens=100000,
        max_duration="30m",
        max_tool_calls=50,
    ),
)

# === MCP Server Integration ===
agent = meerkat.Agent(
    model="claude-sonnet-4-5",
    mcp_servers=[
        meerkat.McpServer.stdio("playwright", "npx", "@anthropic/mcp-playwright"),
        meerkat.McpServer.http("custom-api", "http://localhost:8080/mcp"),
    ],
)

# === Built-in Tools ===
agent = meerkat.Agent(
    model="claude-sonnet-4-5",
    builtins=meerkat.BuiltinTools(
        tasks=True,
        shell=True,
        datetime=True,
        wait=True,
    ),
    project_root="/path/to/project",
)

# === Event Streaming (Callback) ===
def on_event(event: meerkat.AgentEvent):
    match event:
        case meerkat.TextDelta(delta=text):
            print(text, end="", flush=True)
        case meerkat.ToolCallRequested(name=name):
            print(f"\n[Calling {name}...]")

result = agent.run("Write a poem", on_event=on_event)

# === Event Streaming (Sync Iterator) ===
for event in agent.run_iter("Write a poem"):
    if isinstance(event, meerkat.TextDelta):
        print(event.delta, end="")

# === Async API ===
result = await agent.run_async("Hello!")

async for event in agent.run_stream("Write a poem"):
    if isinstance(event, meerkat.TextDelta):
        print(event.delta, end="")

# === Config Management ===
config = meerkat.Config.load()  # Global + local if present
config = meerkat.Config.load(scope="global")
config = meerkat.Config.load(project_root="/path/to/project")

config.models.anthropic = "claude-opus-4-5"
config.max_tokens = 8192

config.save(scope="local")   # Write to .rkat/config.toml
config.save(scope="global")  # Write to ~/.rkat/config.toml

# === Memory-backed Config (SDK default) ===
config = meerkat.Config.memory()
agent = meerkat.Agent(config=config, ...)

# === Session Management ===
sessions = meerkat.SessionManager(".rkat/sessions")

for meta in sessions.list(limit=10):
    print(f"{meta.id}: {meta.message_count} messages")

session = sessions.load("abc-123")
sessions.delete("old-session-id")

# === Context Manager ===
with meerkat.Agent(model="claude-sonnet-4-5") as agent:
    result = agent.run("Hello")
# MCP connections cleaned up

# === Error Handling ===
try:
    result = agent.run("Do something")
except meerkat.BudgetExhausted as e:
    print(f"Ran out of budget: {e.used}/{e.limit} tokens")
except meerkat.RateLimited as e:
    print(f"Rate limited, retry after {e.retry_after}s")
except meerkat.ToolError as e:
    print(f"Tool {e.tool_name} failed: {e.message}")
except meerkat.AgentError as e:
    print(f"Agent error: {e}")
```

### Response Types

```python
@dataclass
class RunResult:
    text: str
    session_id: str
    turns: int
    tool_calls: int
    usage: Usage

@dataclass
class Usage:
    input_tokens: int
    output_tokens: int
    cache_creation_tokens: Optional[int]
    cache_read_tokens: Optional[int]

    @property
    def total_tokens(self) -> int:
        return self.input_tokens + self.output_tokens

# Event types
@dataclass
class TextDelta:
    delta: str

@dataclass
class ToolCallRequested:
    id: str
    name: str
    args: dict

@dataclass
class ToolExecutionCompleted:
    id: str
    name: str
    result: str
    is_error: bool
    duration_ms: int

@dataclass
class TurnCompleted:
    stop_reason: StopReason
    usage: Usage
```

### Implementation Patterns

#### Sync/Async Dual API

```rust
#[pyclass]
pub struct PyAgent {
    inner: Arc<Mutex<Agent<...>>>,
    runtime: tokio::runtime::Runtime,
}

#[pymethods]
impl PyAgent {
    /// Blocking API
    fn run(&self, py: Python<'_>, prompt: String) -> PyResult<PyRunResult> {
        py.allow_threads(|| {
            self.runtime.block_on(async {
                let mut agent = self.inner.lock().await;
                agent.run(prompt).await
            })
        })
        .map(PyRunResult::from)
        .map_err(into_py_err)
    }

    /// Async API
    fn run_async<'py>(&self, py: Python<'py>, prompt: String) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut agent = inner.lock().await;
            agent.run(prompt).await
                .map(PyRunResult::from)
                .map_err(into_py_err)
        })
    }
}
```

#### Callback-based Streaming

```rust
fn run_with_callback(
    &self,
    py: Python<'_>,
    prompt: String,
    callback: PyObject,
) -> PyResult<PyRunResult> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(100);

    py.allow_threads(|| {
        self.runtime.block_on(async {
            let callback_task = tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    Python::with_gil(|py| {
                        let py_event = PyAgentEvent::from(event);
                        let _ = callback.call1(py, (py_event,));
                    });
                }
            });

            let mut agent = self.inner.lock().await;
            let result = agent.run_with_events(prompt, tx).await;
            callback_task.await.ok();
            result
        })
    })
}
```

### Challenges

1. **GIL contention** - Use `py.allow_threads()` for all blocking Rust calls
2. **Async bridging** - `pyo3-async-runtimes` handles tokio↔asyncio
3. **Streaming** - Callback-based is simpler; async iterators require `__aiter__`/`__anext__`

---

## Node.js/TypeScript SDK

### Tooling

- **NAPI-RS** - Rust bindings for Node.js (native addon)
- **napi build** - Build tool for NAPI-RS projects

WASM is not suitable because Meerkat requires file I/O, network, and process spawning.

### Project Structure

```
meerkat-node/
├── Cargo.toml              # napi, napi-derive, tokio
├── package.json            # @meerkat/sdk
├── src/
│   ├── lib.rs              # #[napi] entry point
│   ├── agent.rs            # Agent, AgentBuilder
│   ├── config.rs           # Config, ConfigStore
│   ├── result.rs           # RunResult, Usage
│   ├── events.rs           # AgentEvent, streaming
│   └── sessions.rs         # SessionManager
├── index.js                # Entry point
└── index.d.ts              # Auto-generated TypeScript types
```

### Cargo.toml

```toml
[package]
name = "meerkat-node"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
napi = { version = "2", features = ["async", "napi9"] }
napi-derive = "2"
tokio = { version = "1", features = ["full"] }

# Workspace crates
meerkat-core = { path = "../meerkat-core" }
meerkat-client = { path = "../meerkat-client" }
meerkat-store = { path = "../meerkat-store" }
meerkat-tools = { path = "../meerkat-tools" }
meerkat-agent = { path = "../meerkat-agent" }
meerkat-mcp-client = { path = "../meerkat-mcp-client" }
```

### API Design

```typescript
import { Agent, Config, SessionManager, McpServer, Budget } from '@meerkat/sdk';

// === Simple One-Shot Usage ===
const result = await meerkat.run("What is 2 + 2?");
console.log(result.text);
console.log(result.sessionId);

// === Resume ===
const result2 = await meerkat.resume("session-uuid", "Now multiply by 3");

// === Configuration via Agent Class ===
const agent = new Agent({
  model: "claude-sonnet-4-5",
  systemPrompt: "You are a helpful assistant.",
  maxTokensPerTurn: 4096,
  temperature: 0.7,
  providerParams: { thinking_budget: 10000 },
});

const result = await agent.run("Analyze this code");

// === Budget Configuration ===
const agent = new Agent({
  model: "claude-sonnet-4-5",
  budget: new Budget({
    maxTokens: 100000,
    maxDuration: "30m",
    maxToolCalls: 50,
  }),
});

// === MCP Server Integration ===
const agent = new Agent({
  model: "claude-sonnet-4-5",
  mcpServers: [
    McpServer.stdio("playwright", "npx", ["@anthropic/mcp-playwright"]),
    McpServer.http("custom-api", "http://localhost:8080/mcp"),
  ],
});

// === Built-in Tools ===
const agent = new Agent({
  model: "claude-sonnet-4-5",
  builtins: {
    tasks: true,
    shell: true,
    datetime: true,
    wait: true,
  },
  projectRoot: "/path/to/project",
});

// === Event Streaming (Callback) ===
const result = await agent.run("Write a poem", {
  onEvent: (event) => {
    if (event.type === "text_delta") {
      process.stdout.write(event.delta);
    } else if (event.type === "tool_call_requested") {
      console.log(`\n[Calling ${event.name}...]`);
    }
  },
});

// === Event Streaming (Async Iterator) ===
for await (const event of agent.runStream("Write a poem")) {
  if (event.type === "text_delta") {
    process.stdout.write(event.delta);
  }
}

// === Config Management ===
const config = Config.load();
const config = Config.load({ scope: "global" });
const config = Config.load({ projectRoot: "/path/to/project" });

config.models.anthropic = "claude-opus-4-5";
config.maxTokens = 8192;

config.save({ scope: "local" });
config.save({ scope: "global" });

// === Memory-backed Config ===
const config = Config.memory();
const agent = new Agent({ config });

// === Session Management ===
const sessions = new SessionManager(".rkat/sessions");

for (const meta of await sessions.list({ limit: 10 })) {
  console.log(`${meta.id}: ${meta.messageCount} messages`);
}

const session = await sessions.load("abc-123");
await sessions.delete("old-session-id");

// === Error Handling ===
try {
  const result = await agent.run("Do something");
} catch (e) {
  if (e instanceof meerkat.BudgetExhausted) {
    console.log(`Ran out of budget: ${e.used}/${e.limit} tokens`);
  } else if (e instanceof meerkat.RateLimited) {
    console.log(`Rate limited, retry after ${e.retryAfter}s`);
  } else if (e instanceof meerkat.ToolError) {
    console.log(`Tool ${e.toolName} failed: ${e.message}`);
  }
}
```

### Response Types

```typescript
interface RunResult {
  text: string;
  sessionId: string;
  turns: number;
  toolCalls: number;
  usage: Usage;
}

interface Usage {
  inputTokens: number;
  outputTokens: number;
  cacheCreationTokens?: number;
  cacheReadTokens?: number;
  totalTokens: number;
}

// Event types (discriminated union)
type AgentEvent =
  | { type: "run_started"; sessionId: string }
  | { type: "text_delta"; delta: string }
  | { type: "tool_call_requested"; id: string; name: string; args: object }
  | { type: "tool_execution_completed"; id: string; name: string; result: string; isError: boolean }
  | { type: "turn_completed"; stopReason: StopReason; usage: Usage }
  | { type: "run_completed"; result: RunResult };

type StopReason = "end_turn" | "tool_use" | "max_tokens" | "budget_exhausted";
```

### Implementation Patterns

#### Async Functions

NAPI-RS makes async straightforward since Node.js is async-native:

```rust
use napi::bindgen_prelude::*;
use napi_derive::napi;

#[napi]
pub struct Agent {
    inner: Arc<Mutex<meerkat::Agent<...>>>,
}

#[napi]
impl Agent {
    #[napi(constructor)]
    pub fn new(options: AgentOptions) -> Result<Self> {
        // Build agent from options
    }

    #[napi]
    pub async fn run(&self, prompt: String) -> Result<RunResult> {
        let mut agent = self.inner.lock().await;
        agent.run(prompt).await
            .map(RunResult::from)
            .map_err(into_napi_err)
    }

    #[napi]
    pub async fn resume(&self, session_id: String, prompt: String) -> Result<RunResult> {
        // Resume implementation
    }
}
```

#### Async Iterators for Streaming

Node.js has native async iterator support:

```rust
#[napi]
impl Agent {
    #[napi]
    pub fn run_stream(&self, prompt: String) -> Result<AsyncIterator<AgentEvent>> {
        let (tx, rx) = tokio::sync::mpsc::channel(100);

        // Spawn agent run task
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let mut agent = inner.lock().await;
            let _ = agent.run_with_events(prompt, tx).await;
        });

        Ok(AsyncIterator::new(rx))
    }
}
```

### Advantages Over Python

| Aspect | Python | Node.js |
|--------|--------|---------|
| Async model | GIL + pyo3-async-runtimes | Native async, no bridging |
| Streaming | `__aiter__` complex | `for await` built-in |
| Type generation | Manual .pyi stubs | Auto-generated .d.ts |
| Event loop | Separate tokio runtime | Shares libuv/tokio |

---

## Workspace Integration

Both SDKs live in the main Meerkat repo:

```
raik/
├── Cargo.toml              # workspace members include meerkat-python, meerkat-node
├── meerkat-core/
├── meerkat-client/
├── meerkat-cli/
├── meerkat-rest/
├── meerkat-mcp-server/
├── meerkat-python/         # Python SDK (PyO3)
│   ├── Cargo.toml
│   ├── pyproject.toml
│   └── src/
└── meerkat-node/           # Node.js SDK (NAPI-RS)
    ├── Cargo.toml
    ├── package.json
    └── src/
```

### Workspace Cargo.toml

```toml
[workspace]
members = [
    "meerkat-core",
    "meerkat-client",
    "meerkat-store",
    "meerkat-tools",
    "meerkat-mcp-client",
    "meerkat-mcp-server",
    "meerkat-rest",
    "meerkat-cli",
    "meerkat-agent",
    "meerkat",
    "meerkat-python",
    "meerkat-node",
]
```

### Build Commands

```bash
# Python SDK
cd meerkat-python
maturin develop          # Install locally for development
maturin build --release  # Build wheel

# Node.js SDK
cd meerkat-node
npm run build            # Runs napi build
npm pack                 # Create tarball
```

### CI Integration

Both SDKs should be tested in CI:

```yaml
# Python
- run: cd meerkat-python && maturin develop
- run: cd meerkat-python && pytest

# Node.js
- run: cd meerkat-node && npm ci && npm run build
- run: cd meerkat-node && npm test
```

---

## Implementation Priority

1. **Python SDK** - Larger user base for AI/ML tooling
2. **Node.js SDK** - Strong demand from web/full-stack developers

Both can be developed in parallel as they share no code beyond the Rust workspace crates.

## References

- [PyO3 User Guide](https://pyo3.rs/)
- [pyo3-async-runtimes](https://github.com/PyO3/pyo3-async-runtimes)
- [NAPI-RS Documentation](https://napi.rs/)
- [maturin](https://github.com/PyO3/maturin)
- [Polars Python bindings](https://github.com/pola-rs/polars) (PyO3 reference)
- [napi-rs examples](https://github.com/napi-rs/napi-rs/tree/main/examples)
