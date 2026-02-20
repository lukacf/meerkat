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
rkat-rest --realm team-alpha
rkat-mcp --realm team-alpha
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
For detailed mob behavior across all surfaces, load: `references/mobs.md`.

## Mob behavior (current contract)

- CLI `run`/`resume` compose `mob_*` tools through `meerkat-mob-mcp` dispatcher integration.
- CLI `mob ...` is the explicit lifecycle surface for persisted mob registry operations.
- RPC/REST/MCP server/Python SDK/TypeScript SDK expose mob capability via the same dispatcher composition model (`SessionBuildOptions.external_tools`) in host integrations.
- Member runtime default is `autonomous_host` when `runtime_mode` is omitted; `turn_driven` is explicit opt-in for controlled dispatch paths.

### Mob lifecycle (standard/default usage)

Primary CLI usage is tool-driven through `run`/`resume`:

```bash
rkat run "create a mob with a lead and workers, then wire and report status"
rkat resume <session_id> "retire worker-2 and add worker-4"
```

Where needed, direct lifecycle commands are available as operational compatibility surface:

```bash
rkat mob prefabs
rkat mob create --prefab <name> | --definition <path>
rkat mob list
rkat mob status <mob_id>
rkat mob spawn <mob_id> <profile> <meerkat_id>
rkat mob retire <mob_id> <meerkat_id>
rkat mob wire <mob_id> <a> <b>
rkat mob unwire <mob_id> <a> <b>
rkat mob turn <mob_id> <meerkat_id> <message>
rkat mob stop|resume|complete <mob_id>
rkat mob destroy <mob_id>
```

Mob flows are optional and layered on top of this lifecycle.

### Mob flows (DAG runtime)

Flow commands are now part of the CLI mob lifecycle:

```bash
rkat mob flows <mob_id>
rkat mob run-flow <mob_id> --flow <flow_id> [--params <json-object>]
rkat mob flow-status <mob_id> <run_id>
```

Flow model highlights:

- declarative DAG step graph (`depends_on`),
- dependency mode (`all` or `any`),
- branching via `branch` + `condition`,
- dispatch mode (`one_to_one`, `fan_out`, `fan_in`),
- topology policy enforcement (`strict|permissive` + role rules including `"*"` wildcard),
- persisted run snapshots (`MobRun`) with `step_ledger` and `failure_ledger`.

Operational notes:

- `run-flow` waits until terminal and persists a terminal snapshot.
- `flow-status` checks live run state first and falls back to terminal snapshots.
- flow limits are defined in mob `limits` (`max_flow_duration_ms`, `max_step_retries`, `cancel_grace_timeout_ms`, `max_orphaned_turns`).

Terminology:

- **Mob runtime contract**: where `mob_*` tools and `rkat mob` lifecycle are exposed.
- **Backend selection**: realm-level storage backend (`redb`/`jsonl`) pinned in `realm_manifest.json`.

Do not conflate the two: mob tool availability is a surface behavior, backend is a realm storage choice.

## Quick start

### CLI

```bash
rkat run "What is Rust?"
rkat --realm team-alpha run "Create a todo app" --enable-builtins --enable-shell --stream -v
rkat --realm team-alpha resume sid_abc123 "Now add error handling"
rkat mob prefabs
rkat mob create --prefab coding_swarm
rkat mob list
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

## Feature composition

The `meerkat` facade crate defaults to providers only (Anthropic, OpenAI, Gemini). Everything else is opt-in via Cargo features:

```toml
# Default: three providers, no storage/comms/tools
meerkat = "0.3"

# Single provider, minimal
meerkat = { version = "0.3", default-features = false, features = ["anthropic"] }

# Add persistence + memory + comms
meerkat = { version = "0.3", features = [
    "jsonl-store", "session-store", "session-compaction",
    "memory-store-session", "comms", "mcp", "sub-agents", "skills"
] }
```

Available features: `anthropic`, `openai`, `gemini`, `all-providers`, `jsonl-store`, `memory-store`, `session-store`, `session-compaction`, `memory-store-session`, `comms`, `mcp`, `sub-agents`, `skills`.

Prebuilt binaries (`rkat`, `rkat-rpc`, `rkat-rest`, `rkat-mcp`) include everything. Custom binary builds:

```bash
cargo install rkat --no-default-features --features "anthropic,openai,session-store,mcp"
```

Disabled features return typed errors (e.g. `SessionError::PersistenceDisabled`) — no panics.

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
