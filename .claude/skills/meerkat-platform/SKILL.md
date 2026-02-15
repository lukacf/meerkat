---
name: meerkat-platform
description: "Comprehensive guide for building applications with the Meerkat agent platform. Covers all surfaces (CLI, REST, RPC, MCP, Python SDK, TypeScript SDK, Rust SDK), configuration, sessions, streaming, skills, hooks, sub-agents, memory, and inter-agent communication. This skill should be used when users ask how to integrate with Meerkat, build agents, configure the runtime, use the SDK, set up multi-agent systems, or work with any Meerkat feature."
---

# Meerkat Platform Guide

Meerkat (`rkat`) is a library-first agent runtime exposed through multiple surfaces. The execution pipeline is shared, and state is realm-scoped.

## Realm-first model

`realm_id` is the sharing key across all surfaces.

- Same `realm_id` => shared sessions/config/backend.
- Different `realm_id` => strict isolation.
- Backend (`redb` or `jsonl`) is pinned per realm in `realm_manifest.json`.

Default realm behavior:

- CLI (`run/resume/sessions`): workspace-derived stable realm.
- RPC/REST/MCP/SDK: new opaque realm unless explicitly provided.

Use explicit realm to share:

```bash
rkat --realm team-alpha run "Plan release"
rkat-rpc --realm team-alpha
meerkat-rest --realm team-alpha
meerkat-mcp-server --realm team-alpha
```

## Surfaces

| Surface | Protocol | Use Case |
|---------|----------|----------|
| CLI | Shell commands | Developer workflows, scripting |
| REST | HTTP/JSON | Services and language-agnostic clients |
| RPC | JSON-RPC 2.0 (stdio) | IDE integration and SDK backend |
| MCP | Model Context Protocol | Expose Meerkat as tools |
| Python SDK | Async Python over RPC | Python applications |
| TypeScript SDK | TypeScript over RPC | Node.js applications |
| Rust SDK | Direct library API | Embedded Rust systems |

For full per-surface schemas and examples, load: `references/api_reference.md`.

## Quick start

### CLI

```bash
rkat run "What is Rust?"
rkat --realm team-alpha run "Create a todo app" --enable-builtins --enable-shell --stream -v
rkat --realm team-alpha resume sid_abc123 "Now add error handling"
```

### Python SDK

```python
from meerkat import MeerkatClient

client = MeerkatClient()
await client.connect(realm_id="team-alpha")
result = await client.create_session("What is Rust?")
print(result.text)
await client.close()
```

### TypeScript SDK

```typescript
import { MeerkatClient } from "@meerkat/sdk";

const client = new MeerkatClient();
await client.connect({ realmId: "team-alpha" });
const result = await client.createSession({ prompt: "What is Rust?" });
await client.close();
```

### Rust SDK

```rust
use meerkat::{AgentFactory, AgentBuildConfig};
use meerkat_core::Config;
use meerkat_store;

let config = Config::load().await?;
let realm = meerkat_store::realm_paths("team-alpha");
let factory = AgentFactory::new(realm.root.clone())
    .runtime_root(realm.root)
    .builtins(true)
    .shell(true);
let build = AgentBuildConfig::new("claude-sonnet-4-5");
let mut agent = factory.build_agent(build, &config).await?;
let result = agent.run("What is Rust?".into()).await?;
```

## Configuration

Config APIs return a realm-scoped envelope:

- `config`
- `generation`
- `realm_id`
- `instance_id`
- `backend`
- `resolved_paths`

`config/set` and `config/patch` support `expected_generation` for CAS.

## Core features

### Sessions

Sessions are realm-scoped and surface-neutral. Visibility depends on `realm_id` matching.

### Streaming

Real-time events include `text_delta`, tool lifecycle events, hook events, and terminal run events.

### Skills

Skill loading is runtime-root aware. Workspace realms use project `.rkat/skills`; non-workspace realms use realm runtime roots.

### Hooks

Hook config is realm-aware, with compatibility layering from user/project hook files when available.

### Sub-agents

Sub-agents inherit realm context. With comms enabled, parent/child inproc communication is namespace-scoped by realm.

### Inter-agent comms

Comms supports `inproc`, TCP, and UDS. Inproc registry is namespace-segmented; Meerkat uses realm namespace for isolation.

### Memory

Semantic memory (`memory_search`) and compaction integrate through the same session/runtime pipeline.

## Reference

For complete method signatures and multi-surface examples, load:
`references/api_reference.md`
