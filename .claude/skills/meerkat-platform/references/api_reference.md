# Meerkat Platform API Reference

## CLI Reference

### rkat run

```bash
rkat run <PROMPT> [OPTIONS]

# Model & provider
--model <MODEL>              # e.g. "claude-opus-4-6", "gpt-5.2", "gemini-3-pro-preview"
--provider <PROVIDER>        # anthropic, openai, gemini (auto-inferred from model)

# Budgets
--max-tokens <N>             # Max tokens per turn
--max-total-tokens <N>       # Total token budget
--max-duration <DURATION>    # Time limit: "5m", "1h30m"
--max-tool-calls <N>         # Tool call budget

# Output
--output text|json           # Output format
--stream                     # Stream LLM tokens
--verbose / -v               # Show turn details

# Structured output
--output-schema <SCHEMA>     # JSON schema (file path or inline JSON)
--output-schema-compat lossy|strict
--structured-output-retries <N>  # Default: 2

# Tools
--enable-builtins            # Task management, utilities
--enable-shell               # Shell tool (requires --enable-builtins)
--no-subagents               # Disable sub-agent tools

# Provider params
--param KEY=VALUE            # Repeatable, e.g. --param thinking.budget_tokens=10000

# Hooks
--hooks-override-json <JSON> # Inline hook overrides
--hooks-override-file <FILE> # Hook overrides from file

# Comms & events
--comms-name <NAME>          # Agent name for inter-agent comms
--comms-listen-tcp <ADDR>    # TCP listen address for signed comms
--no-comms                   # Disable comms
--host                       # Host mode: stay alive for messages
--stdin                      # Read external events from stdin (newline-delimited, with --host)
```

### rkat resume

```bash
rkat resume <SESSION-ID> <PROMPT> [--hooks-override-json JSON] [--hooks-override-file FILE]
```

### rkat sessions

```bash
rkat sessions list [--limit N]
rkat sessions show <ID>
rkat sessions delete <ID>
```

### rkat mcp

```bash
rkat mcp add <NAME> -- <COMMAND> [ARGS...]     # stdio
rkat mcp add <NAME> --url <URL>                 # http/sse
rkat mcp list [--scope user|project] [--json]
rkat mcp get <NAME> [--json]
rkat mcp remove <NAME>
```

### rkat config

```bash
rkat config get [--format toml|json]
rkat config set --file config.toml
rkat config patch --json '{"budget":{"max_tokens":50000}}'
```

---

## REST API

### POST /sessions — Create & run session

```bash
curl -X POST http://localhost:3000/sessions \
  -H "Content-Type: application/json" \
  -d '{
    "prompt": "Analyze this code",
    "model": "claude-sonnet-4-5",
    "enable_builtins": true,
    "enable_shell": true,
    "max_tokens": 4096
  }'
```

Response: `WireRunResult` with `session_id`, `text`, `turns`, `tool_calls`, `usage`.

### POST /sessions/{id}/messages — Continue session

```bash
curl -X POST http://localhost:3000/sessions/sid_abc/messages \
  -d '{"prompt": "Now add tests"}'
```

### GET /sessions/{id}/events — SSE stream

```bash
curl -N http://localhost:3000/sessions/sid_abc/events
```

### POST /sessions/{id}/event — Push external event

```bash
# Without auth (localhost only)
curl -X POST http://localhost:3000/sessions/sid_abc/event \
  -H "Content-Type: application/json" \
  -d '{"source": "github", "action": "push", "ref": "main"}'

# With webhook secret (set RKAT_WEBHOOK_SECRET on server)
curl -X POST http://localhost:3000/sessions/sid_abc/event \
  -H "Content-Type: application/json" \
  -H "X-Webhook-Secret: my-secret" \
  -d '{"alert": "deployment failed"}'
```

Response: `202 ACCEPTED` with `{"queued": true}`. Returns `503` if inbox full, `404` if session not found. Auth via `RKAT_WEBHOOK_SECRET` env var with constant-time comparison.

### Other endpoints

- `GET /sessions/{id}` — Session details
- `GET /health` — Health check
- `GET /capabilities` — Runtime capabilities
- `GET /config` / `PUT /config` / `PATCH /config` — Config management

---

## JSON-RPC (via `rkat rpc`)

All methods use JSON-RPC 2.0 over newline-delimited JSON on stdin/stdout.

### session/create

```json
{"jsonrpc":"2.0","id":1,"method":"session/create","params":{
  "prompt": "Hello!",
  "model": "claude-sonnet-4-5",
  "enable_builtins": true,
  "enable_shell": true,
  "preload_skills": ["extraction/email"],
  "skill_references": ["formatting/markdown"]
}}
```

### turn/start

```json
{"jsonrpc":"2.0","id":2,"method":"turn/start","params":{
  "session_id": "sid_abc123",
  "prompt": "Now add error handling",
  "skill_references": ["rust-patterns/error-handling"]
}}
```

### Notifications (server -> client)

```json
{"jsonrpc":"2.0","method":"session/event","params":{
  "session_id": "sid_abc123",
  "event": {"type": "text_delta", "delta": "Here's"}
}}
```

### event/push

```json
{"jsonrpc":"2.0","id":3,"method":"event/push","params":{
  "session_id": "sid_abc123",
  "payload": {"alert": "build failed", "repo": "main"},
  "source": "ci-pipeline"
}}
```

Response: `{"result": {"queued": true}}`. The `payload` is any JSON value. Optional `source` is prepended as `[source: ci-pipeline]` metadata. Error `-32603` if inbox full, `-32602` if session not found.

### Other methods

- `initialize {}` — Handshake
- `session/list {}` — List sessions
- `session/read {session_id}` — Read session
- `session/archive {session_id}` — Archive session
- `turn/interrupt {session_id}` — Cancel turn
- `config/get {}` / `config/set {config}` / `config/patch {patch}`
- `capabilities/get {}` — Runtime capabilities

---

## MCP Server

Exposed as MCP tools when running Meerkat as an MCP server:

### meerkat_run

```json
{
  "prompt": "Analyze this code",
  "model": "claude-sonnet-4-5",
  "enable_builtins": true,
  "tools": [
    {"name": "read_file", "description": "Read a file", "inputSchema": {...}}
  ]
}
```

### meerkat_resume

```json
{
  "session_id": "sid_abc",
  "prompt": "Continue with tests",
  "tool_results": [
    {"tool_use_id": "tu_123", "content": "file contents...", "is_error": false}
  ]
}
```

### meerkat_config / meerkat_capabilities

Config management and capability discovery as MCP tools.

---

## Python SDK

### Connection

```python
from meerkat import MeerkatClient

client = MeerkatClient(rkat_path="rkat")  # or custom path
await client.connect()  # starts rkat rpc subprocess + handshake
# ... use client ...
await client.close()
```

### Session Methods

```python
# Create session (non-streaming)
result = await client.create_session(
    prompt="Hello!",
    model="claude-opus-4-6",
    provider="anthropic",
    system_prompt="You are a helpful assistant",
    max_tokens=4096,
    enable_builtins=True,
    enable_shell=True,
    enable_subagents=True,
    enable_memory=True,
    preload_skills=["extraction/email"],
    provider_params={"thinking": {"budget_tokens": 10000}},
)

# Continue session
result = await client.start_turn(
    session_id=result.session_id,
    prompt="Now add tests",
    skill_references=["testing/pytest"],
)

# Interrupt
await client.interrupt(session_id)

# Session management
sessions = await client.list_sessions()
details = await client.read_session(session_id)
await client.archive_session(session_id)

# Push external event into running session
await client.push_event(
    session_id=session_id,
    payload={"alert": "deployment failed", "env": "prod"},
    source="monitoring",  # optional
)
```

### Streaming

```python
# Stream events during creation
async with client.create_session_streaming("Write a REST API") as stream:
    async for event in stream:
        match event["type"]:
            case "text_delta":
                print(event["delta"], end="", flush=True)
            case "tool_call_requested":
                print(f"\n[Calling: {event['name']}]")
            case "turn_completed":
                print(f"\n--- Turn done ---")
    result = stream.result

# Stream during turn
async with client.start_turn_streaming(session_id, "Add auth") as stream:
    full_text, result = await stream.collect_text()

# Consume silently
async with client.create_session_streaming("Summarize") as stream:
    result = await stream.collect()
```

### Capabilities

```python
caps = await client.get_capabilities()
if client.has_capability("shell"):
    print("Shell available")
client.require_capability("skills")  # raises if unavailable
```

### Config

```python
config = await client.get_config()
await client.set_config({"budget": {"max_tokens": 50000}})
updated = await client.patch_config({"agent": {"model": "claude-opus-4-6"}})
```

### Structured Output

```python
result = await client.create_session(
    "Extract entities from: 'John works at Acme Corp in NYC'",
    output_schema={
        "schema": {
            "type": "object",
            "properties": {
                "person": {"type": "string"},
                "company": {"type": "string"},
                "location": {"type": "string"}
            },
            "required": ["person", "company", "location"]
        },
        "name": "entities"
    },
    structured_output_retries=3,
)
print(result.structured_output)
# {"person": "John", "company": "Acme Corp", "location": "NYC"}
```

---

## TypeScript SDK

### Connection

```typescript
import { MeerkatClient } from "@meerkat/sdk";

const client = new MeerkatClient("rkat");
await client.connect();
// ... use client ...
await client.close();
```

### Session Methods

```typescript
// Create session
const result = await client.createSession({
  prompt: "Build a dashboard",
  model: "claude-sonnet-4-5",
  enable_builtins: true,
  enable_shell: true,
  preload_skills: ["frontend/react"],
  skill_references: ["design-system/components"],
});

// Continue session
const next = await client.startTurn(result.session_id, "Add dark mode", {
  skill_references: ["design-system/themes"],
});

// Interrupt / list / read / archive
await client.interrupt(sessionId);
const sessions = await client.listSessions();
const details = await client.readSession(sessionId);
await client.archiveSession(sessionId);

// Push external event
await client.pushEvent(sessionId, {
  payload: { alert: "build failed" },
  source: "ci",
});
```

### Capabilities & Config

```typescript
const caps = await client.getCapabilities();
const checker = new CapabilityChecker(caps);
checker.require("skills");

const config = await client.getConfig();
await client.patchConfig({ budget: { max_tokens: 50000 } });
```

---

## Rust SDK

### AgentFactory

```rust
use meerkat::{AgentFactory, AgentBuildConfig};
use meerkat_core::Config;

let config = Config::load().await?;

let factory = AgentFactory::new(".rkat/sessions")
    .project_root(std::env::current_dir()?)
    .builtins(true)
    .shell(true)
    .subagents(true)
    .memory(true)
    .comms(true);

// Or with a custom session store (e.g. BigQuery, DynamoDB):
let custom_store: Arc<dyn SessionStore> = Arc::new(MySessionStore::new());
let factory = AgentFactory::new(".rkat/sessions")
    .session_store(custom_store)  // overrides feature-flag default
    .builtins(true)
    .shell(true);

let build = AgentBuildConfig {
    model: "claude-opus-4-6".into(),
    provider: Some(Provider::Anthropic),
    max_tokens: Some(8192),
    system_prompt: Some("You are a code reviewer".into()),
    budget_limits: Some(BudgetLimits {
        max_tokens: Some(100_000),
        max_duration: Some(Duration::from_secs(1800)),
        max_tool_calls: Some(50),
    }),
    preload_skills: Some(vec![SkillId("code-review/rust".into())]),
    ..AgentBuildConfig::new("claude-opus-4-6")
};

let mut agent = factory.build_agent(build, &config).await?;
```

### Running & Streaming

```rust
// Simple run
let result = agent.run("Review this PR".into()).await?;

// With events
let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(100);
tokio::spawn(async move {
    while let Some(event) = event_rx.recv().await {
        match &event {
            AgentEvent::TextDelta { delta, .. } => print!("{delta}"),
            AgentEvent::ToolCallRequested { name, .. } => println!("[Tool: {name}]"),
            _ => {}
        }
    }
});
let result = agent.run_with_events("Review this PR".into(), event_tx).await?;
```

### Session Service

```rust
use meerkat::EphemeralSessionService;

let service = EphemeralSessionService::new(builder, max_concurrent);

let result = service.create_session(CreateSessionRequest {
    model: "claude-sonnet-4-5".into(),
    prompt: "Hello".into(),
    system_prompt: None,
    max_tokens: None,
    event_tx: None,
    host_mode: false,
    skill_references: None,
}).await?;

let turn_result = service.start_turn(&session_id, StartTurnRequest {
    prompt: "Continue".into(),
    event_tx: None,
    host_mode: false,
    skill_references: Some(vec![SkillId("review/security".into())]),
}).await?;
```

### Interaction-Scoped Event Streaming

Inject a message into a host-mode agent and subscribe to the streaming events from the turn that processes it.

```rust
use meerkat::{SubscribableInjector, AgentEvent, PlainEventSource};

// Get the subscribable injector for a running session
let injector = service.event_injector(&session_id).await
    .expect("comms must be enabled");

// Fire-and-forget (existing)
injector.inject("hello".into(), PlainEventSource::Rpc)?;

// With subscription — dedicated event stream for this interaction
let sub = injector.inject_with_subscription(
    "review PR #42".into(),
    PlainEventSource::Rpc,
)?;
// sub.id     — InteractionId (for logging/correlation)
// sub.events — mpsc::Receiver<AgentEvent> (scoped to this interaction)

while let Some(event) = sub.events.recv().await {
    match event {
        AgentEvent::TextDelta { delta } => print!("{delta}"),
        AgentEvent::InteractionComplete { result, .. } => {
            println!("\n{result}");
            break; // terminal — stream done
        }
        AgentEvent::InteractionFailed { error, .. } => {
            eprintln!("error: {error}");
            break; // terminal
        }
        _ => {} // TurnStarted, ToolCallRequested, etc.
    }
}
```

Guarantees: no cross-talk between concurrent interactions, exactly one terminal event, backpressure drops intermediate events (with `StreamTruncated` marker) but never the terminal. Requires host mode with a configured primary `event_tx` (typically `AgentBuildConfig.event_tx` + `run_host_mode()`) and comms enabled.

---

## Kitchen-Sink Examples

### Example 1: Multi-turn code review with streaming, skills, and hooks (Python)

```python
from meerkat import MeerkatClient

client = MeerkatClient()
await client.connect()

# Create session with preloaded skills, builtins, and shell
async with client.create_session_streaming(
    "Review the authentication module in src/auth/",
    model="claude-opus-4-6",
    enable_builtins=True,
    enable_shell=True,
    preload_skills=["code-review/security", "rust-patterns/error-handling"],
    hooks_override={"cost_guard": {"enabled": False}},  # disable cost guard for thorough review
    provider_params={"thinking": {"budget_tokens": 20000}},
) as stream:
    findings = []
    async for event in stream:
        if event["type"] == "text_delta":
            print(event["delta"], end="", flush=True)
        elif event["type"] == "tool_call_requested":
            print(f"\n[Reading: {event.get('args', {}).get('path', '?')}]")
    initial_result = stream.result

session_id = initial_result.session_id
print(f"\n\nSession: {session_id}")
print(f"Tokens used: {initial_result.usage.total_tokens}")

# Second turn: ask for fixes with a different skill
result = await client.start_turn(
    session_id,
    "Now fix the critical issues you found. Write the corrected code.",
    skill_references=["rust-patterns/ownership"],
)
print(f"Fixes applied across {result.tool_calls} files")

# Third turn: generate tests
async with client.start_turn_streaming(session_id, "Write comprehensive tests for the fixes") as stream:
    test_code, result = await stream.collect_text()
    print(f"Generated {len(test_code)} bytes of test code")

# Check session history
details = await client.read_session(session_id)
sessions = await client.list_sessions()
print(f"Total sessions: {len(sessions)}")

await client.close()
```

### Example 2: Structured extraction pipeline with budget control (CLI + Python)

```bash
# CLI: Extract with schema, budget, and JSON output
rkat run "Extract all API endpoints from the codebase" \
  --enable-builtins --enable-shell \
  --output-schema '{"type":"object","properties":{"endpoints":{"type":"array","items":{"type":"object","properties":{"method":{"type":"string"},"path":{"type":"string"},"handler":{"type":"string"}},"required":["method","path","handler"]}}},"required":["endpoints"]}' \
  --structured-output-retries 3 \
  --max-total-tokens 50000 \
  --max-duration 5m \
  --output json
```

```python
# Python: Same thing programmatically with streaming progress
import json

schema = {
    "type": "object",
    "properties": {
        "endpoints": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "method": {"type": "string"},
                    "path": {"type": "string"},
                    "handler": {"type": "string"},
                    "auth_required": {"type": "boolean"}
                },
                "required": ["method", "path", "handler"]
            }
        }
    },
    "required": ["endpoints"]
}

async with client.create_session_streaming(
    "Find and document all API endpoints in the codebase",
    enable_builtins=True,
    enable_shell=True,
    output_schema={"schema": schema, "name": "api_endpoints"},
    structured_output_retries=3,
) as stream:
    tool_count = 0
    async for event in stream:
        if event["type"] == "tool_call_requested":
            tool_count += 1
            print(f"  Scanning ({tool_count})... {event.get('name', '')}")
        elif event["type"] == "text_delta":
            pass  # structured output is in result.structured_output
    result = stream.result

endpoints = result.structured_output["endpoints"]
print(f"\nFound {len(endpoints)} endpoints:")
for ep in endpoints:
    auth = " [AUTH]" if ep.get("auth_required") else ""
    print(f"  {ep['method']:6s} {ep['path']}{auth} -> {ep['handler']}")
```

### Example 3: Multi-agent system with host mode, comms, and external events (CLI)

```bash
# Terminal 1: Start a coordinator agent in host mode with stdin events
rkat run "You are a project coordinator. Delegate tasks to specialist agents." \
  --comms-name coordinator \
  --comms-listen-tcp 0.0.0.0:4200 \
  --host --stdin \
  --enable-builtins \
  -v

# Terminal 2: Start a code reviewer agent
rkat run "You are a code reviewer. Wait for review requests." \
  --comms-name reviewer \
  --comms-listen-tcp 0.0.0.0:4201 \
  --host \
  --enable-builtins --enable-shell

# Terminal 3: Send a task to the coordinator via inter-agent comms
rkat run "Tell the coordinator to review src/auth.rs and write tests for it" \
  --comms-name client

# Terminal 4: Push an external event via stdin (in Terminal 1)
echo '{"body":"CI pipeline failed on main branch"}' # typed into coordinator's stdin

# Or push via netcat to a plain event listener (requires auth=none + event_address in config)
echo '{"body":"deployment alert"}' | nc 127.0.0.1 4201
```

### Example 4: Full-stack application with sub-agents (Rust SDK)

```rust
use meerkat::{AgentFactory, AgentBuildConfig};
use meerkat_core::{Config, AgentEvent};
use tokio::sync::mpsc;

let config = Config::load().await?;
let factory = AgentFactory::new(".rkat/sessions")
    .builtins(true)
    .shell(true)
    .subagents(true)  // Enable sub-agent tools
    .memory(true);     // Enable semantic memory

let build = AgentBuildConfig {
    model: "claude-opus-4-6".into(),
    system_prompt: Some(
        "You are a full-stack architect. Use sub-agents to parallelize work. \
         Use agent_spawn for independent tasks and agent_fork for tasks that \
         need conversation context. Use memory_search to recall past decisions."
            .into(),
    ),
    budget_limits: Some(BudgetLimits {
        max_tokens: Some(500_000),
        max_duration: Some(Duration::from_secs(3600)),
        max_tool_calls: Some(200),
    }),
    preload_skills: Some(vec![
        SkillId("architecture/microservices".into()),
        SkillId("devops/docker".into()),
    ]),
    ..AgentBuildConfig::new("claude-opus-4-6")
};

// Stream events
let (tx, mut rx) = mpsc::channel(100);
tokio::spawn(async move {
    while let Some(event) = rx.recv().await {
        match &event {
            AgentEvent::TextDelta { delta, .. } => print!("{delta}"),
            AgentEvent::ToolCallRequested { name, args, .. } => {
                if name == "agent_spawn" || name == "agent_fork" {
                    println!("\n[Spawning sub-agent...]");
                }
            }
            AgentEvent::RunCompleted { usage, .. } => {
                println!("\n\nTotal: {} tokens", usage.total_tokens);
            }
            _ => {}
        }
    }
});

let mut agent = factory.build_agent(build, &config).await?;
let result = agent
    .run_with_events(
        "Design and implement a microservices architecture for an e-commerce platform \
         with user service, product catalog, order processing, and payment gateway. \
         Use sub-agents to implement each service in parallel."
            .into(),
        tx,
    )
    .await?;

println!("Created {} files across {} tool calls", result.tool_calls, result.turns);
```

### Example 5: Dynamic config + capability checking (TypeScript)

```typescript
import { MeerkatClient, CapabilityChecker } from "@meerkat/sdk";

const client = new MeerkatClient();
await client.connect();

// Check capabilities before using features
const caps = await client.getCapabilities();
const checker = new CapabilityChecker(caps);

const features: string[] = [];
if (checker.has("builtins")) features.push("task management");
if (checker.has("shell")) features.push("shell access");
if (checker.has("skills")) features.push("skill loading");
if (checker.has("memory_store")) features.push("semantic memory");
if (checker.has("comms")) features.push("inter-agent comms");
console.log(`Available: ${features.join(", ")}`);

// Dynamically adjust config based on workload
await client.patchConfig({
  budget: { max_tokens: 200000, max_duration: "1h" },
  agent: { model: "claude-opus-4-6" },
});

// Create session with all available features
const result = await client.createSession({
  prompt: "Build a complete CI/CD pipeline for this Rust project",
  enable_builtins: checker.has("builtins"),
  enable_shell: checker.has("shell"),
  enable_subagents: checker.has("sub_agents"),
  enable_memory: checker.has("memory_store"),
  preload_skills: checker.has("skills")
    ? ["rust-cicd-pipeline", "devops/docker"]
    : undefined,
  provider_params: {
    thinking: { budget_tokens: 15000 },
  },
});

console.log(`Session: ${result.session_id}`);
console.log(`Response: ${result.text.slice(0, 200)}...`);
console.log(`Tokens: ${result.usage.total_tokens}`);

// Multi-turn with skill injection
const followup = await client.startTurn(
  result.session_id,
  "Now add security scanning to the pipeline",
  { skill_references: ["security/sast", "security/dependency-audit"] },
);

await client.close();
```

### Example 6: REST webhook handler with SSE streaming (curl/Node.js)

```bash
# Start REST server
rkat rest --port 3000

# Create session and stream events
SESSION=$(curl -s -X POST http://localhost:3000/sessions \
  -H "Content-Type: application/json" \
  -d '{"prompt":"Build a webhook handler","enable_builtins":true,"enable_shell":true}' \
  | jq -r '.session_id')

# Stream events in another terminal
curl -N http://localhost:3000/sessions/$SESSION/events

# Continue the session
curl -s -X POST http://localhost:3000/sessions/$SESSION/messages \
  -d '{"prompt":"Add rate limiting and retry logic"}'
```

```javascript
// Node.js: REST client with SSE
const response = await fetch("http://localhost:3000/sessions", {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify({
    prompt: "Analyze server logs for anomalies",
    enable_builtins: true,
    output_schema: {
      schema: {
        type: "object",
        properties: {
          anomalies: { type: "array", items: { type: "object" } },
          severity: { type: "string", enum: ["low", "medium", "high", "critical"] }
        }
      },
      name: "log_analysis"
    }
  })
});
const result = await response.json();
console.log("Anomalies found:", result.structured_output.anomalies.length);
```

---

## Configuration Reference

### Full config.toml template

```toml
[agent]
model = "claude-sonnet-4-5"
max_tokens_per_turn = 8192
# system_prompt = "Custom system prompt"

[budget]
max_tokens = 100000
max_duration = "30m"
max_tool_calls = 50

[shell]
timeout_secs = 120
security_mode = "permissive"
# program = "/bin/bash"

[comms]
mode = "inproc"                    # "inproc", "tcp", or "uds"
# address = "127.0.0.1:4200"      # Signed agent-to-agent listener
# auth = "none"                    # "none" (open) or "ed25519"
# event_address = "127.0.0.1:4201" # Plain-text external event listener (requires auth = "none")
auto_enable_for_subagents = false

[skills]
enabled = true
max_injection_bytes = 32768
inventory_threshold = 12

[hooks]
default_timeout_ms = 5000
# max_payload_bytes = 65536

[rest]
host = "127.0.0.1"
port = 3000

[sub_agents]
default_provider = "inherit"
default_model = "inherit"
```

### skills.toml

```toml
enabled = true
max_injection_bytes = 32768
inventory_threshold = 12

[[repositories]]
name = "company-skills"
transport = "git"
url = "https://github.com/company/meerkat-skills.git"
git_ref = "main"
ref_type = "branch"
refresh_seconds = 300
depth = 1
# auth_token = "${GITHUB_TOKEN}"
# skills_root = "skills"

[[repositories]]
name = "remote-api"
transport = "http"
url = "https://skills.example.com/api"
# auth_bearer = "${API_KEY}"
cache_ttl_seconds = 300
```

### hooks.toml

```toml
[[hooks]]
id = "cost-guard"
point = "turn_boundary"
capability = "guardrail"
execution_mode = "foreground"
failure_policy = "fail-closed"
command = ["python", ".rkat/hooks/cost_guard.py"]
timeout_ms = 3000

[[hooks]]
id = "audit-logger"
point = "post_tool_execution"
capability = "observe"
execution_mode = "background"
command = ["python", ".rkat/hooks/audit.py"]
```

---

## Event Types Reference

| Event Type | Fields | Description |
|-----------|--------|-------------|
| `run_started` | `session_id`, `prompt` | Agent execution began |
| `turn_started` | `turn_number` | New LLM turn started |
| `text_delta` | `delta` | Incremental text output |
| `text_complete` | `content` | Full text for turn |
| `tool_call_requested` | `name`, `id`, `args` | Tool call dispatched |
| `tool_execution_started` | `name`, `id` | Tool began executing |
| `tool_execution_completed` | `name`, `id`, `result` | Tool finished |
| `tool_execution_timed_out` | `name`, `id` | Tool exceeded timeout |
| `tool_result_received` | `name`, `result` | Tool result processed |
| `turn_completed` | `usage`, `stop_reason` | LLM turn finished |
| `run_completed` | `usage` | Agent run finished |
| `run_failed` | `error` | Agent run failed |
| `budget_warning` | `reason` | Budget threshold reached |
| `compaction_started` | | Context compaction began |
| `compaction_completed` | `removed`, `retained` | Compaction finished |
| `retrying` | `attempt`, `delay_ms`, `error` | Retrying after error |
| `hook_started` | `hook_id`, `point` | Hook execution began |
| `hook_completed` | `hook_id`, `decision` | Hook finished |
| `hook_denied` | `hook_id`, `reason` | Hook blocked operation |
| `skills_resolved` | `ids` | Skills loaded for turn |

---

## Error Codes

### Client-side (SDK)

| Code | Description |
|------|-------------|
| `BINARY_NOT_FOUND` | `rkat` not on PATH |
| `NOT_CONNECTED` | Client not connected |
| `CONNECTION_CLOSED` | Process exited |
| `VERSION_MISMATCH` | Contract version incompatible |
| `CAPABILITY_UNAVAILABLE` | Required capability missing |
| `STREAM_NOT_CONSUMED` | Accessed result before iterating |

### Server-side (JSON-RPC)

| Code | Number | Description |
|------|--------|-------------|
| `INVALID_PARAMS` | -32602 | Bad parameters |
| `SESSION_NOT_FOUND` | -32001 | Session doesn't exist |
| `SESSION_BUSY` | -32002 | Turn in progress |
| `PROVIDER_ERROR` | -32010 | LLM provider error |
| `BUDGET_EXHAUSTED` | -32011 | Budget exceeded |
| `HOOK_DENIED` | -32012 | Hook blocked operation |
| `AGENT_ERROR` | -32013 | Internal agent error |

### REST

| Code | HTTP Status | Description |
|------|-------------|-------------|
| `BAD_REQUEST` | 400 | Invalid request |
| `NOT_FOUND` | 404 | Session not found |
| `CONFIGURATION_ERROR` | 500 | Config issue |
| `AGENT_ERROR` | 500 | Agent failure |
