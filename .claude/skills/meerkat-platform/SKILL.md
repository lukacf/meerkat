---
name: meerkat-platform
description: "Comprehensive guide for building applications with the Meerkat agent platform. Covers all surfaces (CLI, REST, RPC, MCP, Python SDK, TypeScript SDK, Rust SDK), configuration, sessions, streaming, skills, hooks, sub-agents, memory, and inter-agent communication. This skill should be used when users ask how to integrate with Meerkat, build agents, configure the runtime, use the SDK, set up multi-agent systems, or work with any Meerkat feature."
---

# Meerkat Platform Guide

Meerkat (`rkat`) is a minimal, high-performance agent harness for LLM-powered applications. It provides the core execution loop, tool dispatch, session management, and multi-agent coordination without opinions about prompts or output formatting.

## Surfaces

Meerkat exposes the same agent engine through multiple surfaces. All share the same underlying `AgentFactory::build_agent()` pipeline.

| Surface | Protocol | Use Case |
|---------|----------|----------|
| CLI | Shell commands | Developer workflows, scripting |
| REST | HTTP/JSON | Web services, microservices |
| RPC | JSON-RPC 2.0 (stdio) | IDE integration, SDK backend |
| MCP | Model Context Protocol | LLM tool integration |
| Python SDK | Async Python over RPC | Python applications |
| TypeScript SDK | TypeScript over RPC | Node.js applications |
| Rust SDK | Direct library calls | Embedded Rust applications |

For full API reference for each surface including method signatures, parameters, and complex multi-step examples, load:
`references/api_reference.md`

## Quick Start

### CLI

```bash
rkat run "What is Rust?"
rkat run "Create a todo app" --enable-builtins --enable-shell --stream -v
rkat resume sid_abc123 "Now add error handling"
```

### Python SDK

```python
from meerkat import MeerkatClient

client = MeerkatClient()
await client.connect()
result = await client.create_session("What is Rust?")
print(result.text)

# Streaming
async with client.create_session_streaming("Write a poem") as stream:
    async for event in stream:
        if event["type"] == "text_delta":
            print(event["delta"], end="", flush=True)
    result = stream.result

await client.close()
```

### TypeScript SDK

```typescript
import { MeerkatClient } from "@meerkat/sdk";
const client = new MeerkatClient();
await client.connect();
const result = await client.createSession({ prompt: "What is Rust?" });
await client.close();
```

### Rust SDK

```rust
use meerkat::{AgentFactory, AgentBuildConfig};
use meerkat_core::Config;

let config = Config::load().await?;
let factory = AgentFactory::new(".rkat/sessions").builtins(true).shell(true);
let build = AgentBuildConfig::new("claude-sonnet-4-5");
let mut agent = factory.build_agent(build, &config).await?;
let result = agent.run("What is Rust?".into()).await?;
```

## Configuration

Configuration loads from `.rkat/config.toml` (project) or `~/.rkat/config.toml` (global). API keys come from environment variables only.

```toml
[agent]
model = "claude-sonnet-4-5"
max_tokens_per_turn = 8192

[budget]
max_tokens = 100000
max_duration = "30m"
max_tool_calls = 50

[shell]
timeout_secs = 120

[comms]
mode = "inproc"           # "inproc", "tcp", or "uds"
# address = "127.0.0.1:4200"      # Signed agent-to-agent listener
# auth = "none"                    # "none" (open) or "ed25519"
# event_address = "127.0.0.1:4201" # Plain-text external event listener
# auto_enable_for_subagents = false

[skills]
enabled = true
max_injection_bytes = 32768
inventory_threshold = 12
```

Environment variables: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`.

## Core Features

### Sessions

Sessions persist conversation state across turns. Lifecycle: create -> turn -> turn -> ... -> archive.

### Streaming

Real-time event streaming: `text_delta`, `tool_call_requested`, `tool_result_received`, `turn_completed`, `run_completed`. Python SDK provides `create_session_streaming()` and `start_turn_streaming()` with async context manager pattern.

### Skills

Domain-specific knowledge injection. Activated via `/skill-ref` in messages, `skill_references` wire param, or `preload_skills` at session creation. Sources: filesystem, git, HTTP, embedded. Configure in `.rkat/skills.toml`.

### Hooks

8 lifecycle hook points (RunStarted, PreLlmRequest, PostLlmResponse, PreToolExecution, PostToolExecution, TurnBoundary, RunCompleted, RunFailed) for policy enforcement, logging, and content modification.

### Sub-Agents

Hierarchical agent spawning with tools: `agent_spawn`, `agent_fork`, `agent_status`, `agent_cancel`, `agent_list`. Budget-isolated with configurable model inheritance.

### Inter-Agent Communication & External Events

**Agent-to-agent comms:** Ed25519-signed peer-to-peer messaging via `send_message`, `send_request`, `send_response`, `list_peers`. Host mode (`--host`) keeps agent alive listening for messages. Signed listeners use CBOR + Ed25519 with trusted peer verification.

**External event ingestion:** External systems (webhooks, scripts, stdin) can push events into a running agent's inbox. All events flow through the `EventInjector` trait into a bounded inbox, drained at turn boundaries (or continuously in host mode) and injected as user messages.

| Surface | Ingestion Method | Source Tag | Auth |
|---------|-----------------|------------|------|
| CLI | `--stdin` (newline-delimited) | `stdin` | None |
| REST | `POST /sessions/{id}/event` | `webhook` | `RKAT_WEBHOOK_SECRET` header |
| RPC | `event/push` method | `rpc` | None (implicit) |
| Comms | TCP/UDS plain listeners | `tcp`/`uds` | `auth = "none"` required |

**Auth model:** Signed (Ed25519) listeners always run for agent-to-agent comms. When `auth = "none"`, a separate plain-text listener starts on `event_address` for unauthenticated external events. The signed listener is never replaced. REST webhook auth uses `RKAT_WEBHOOK_SECRET` env var with constant-time comparison (`subtle::ConstantTimeEq`).

### Memory

Semantic memory indexes conversation history for cross-session retrieval via `memory_search` tool. Uses HNSW vectors backed by redb.

### Structured Output

JSON schema-based extraction with automatic retry on validation failure. Pass `output_schema` on any surface.

## Reference

For complete API reference with all method signatures, parameters, return types, and complex multi-feature examples across all surfaces, load: `references/api_reference.md`
