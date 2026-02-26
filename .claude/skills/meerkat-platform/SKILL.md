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
- Spawned mob members use deferred initial turn semantics; mob actor lifecycle starts autonomous loops explicitly after spawn registration.
- Portable mob artifacts are available through mobpack (`rkat mob pack/deploy/inspect/validate`) and browser deployment (`rkat mob web build`).

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

### Mobpack + web build quick paths

```bash
# Build portable artifact
rkat mob pack ./mobs/release-triage -o ./dist/release-triage.mobpack --sign ./keys/release.key
rkat mob inspect ./dist/release-triage.mobpack
rkat mob validate ./dist/release-triage.mobpack

# Deploy with trust policy
rkat mob deploy ./dist/release-triage.mobpack "triage latest regressions" --trust-policy strict

# Browser bundle
cargo install wasm-pack
rkat mob web build ./dist/release-triage.mobpack -o ./dist/release-triage-web
```

Web build env overrides:

- `RKAT_WASM_PACK_BIN`: explicit wasm-pack binary path
- `RKAT_WEB_RUNTIME_CRATE_DIR`: explicit web runtime crate directory for build

### WASM runtime (embedded deployment target)

The `meerkat-web-runtime` crate is an **embedded deployment target for mobpacks** — the JavaScript equivalent of the Rust SDK. It compiles the real meerkat agent stack to wasm32, routing through the same `AgentFactory::build_agent()` pipeline as all other surfaces.

**Not a protocol server** — unlike RPC/REST, the WASM runtime is deployed INTO a host application (browser, Node.js). A mobpack defines what it is. The host provides credentials and drives interaction.

**Architecture:**
- Routes through `EphemeralSessionService<FactoryAgentBuilder>` → `AgentFactory::build_agent()` with override-first resource injection
- `MobMcpState` handles all mob lifecycle operations (same state manager as native MCP mob surface)
- `tokio_with_wasm` provides the async runtime (single-threaded, JS event loop backed)
- `reqwest` uses browser `fetch` on wasm32
- `InprocRegistry` provides peer discovery for comms (all sessions share one process-global registry)
- 9 crates compile for wasm32 (store, skills, hooks, comms, tools, session, facade, mob, mob-mcp)

**Fully available on wasm32:**
- Agent loop (streaming, retries, error recovery, budget enforcement)
- All three LLM providers (Anthropic, OpenAI, Gemini) via browser fetch
- Session lifecycle (EphemeralSessionService)
- Mob orchestration (MobBuilder, MobActor, FlowEngine, in-memory storage)
- Comms (inproc — InprocRegistry, Ed25519 signing, peer discovery)
- Tool dispatch (task tools, comms tools, skill tools — no shell)
- Skills (embedded + memory sources from mobpack)
- Hooks (in-process + HTTP — no command hooks)
- Config (in-memory, programmatic)
- Compaction (DefaultCompactor)

**Not available on wasm32 (inherent browser limitations):**
- Filesystem (config loading, AGENTS.md, skill files, session persistence)
- Shell tool, process spawning, sub-agent spawning
- MCP protocol client (rmcp blocked by tokio/mio)
- Network comms (TCP/UDS sockets — inproc only)

**API surface (25 wasm_bindgen exports):**
```
# Bootstrap
init_runtime(mobpack_bytes, credentials_json)
init_runtime_from_config(config_json)

# Sessions
create_session(mobpack_bytes, config_json) → handle
start_turn(handle, prompt, options_json) → RunResult JSON  [async]
get_session_state(handle) → JSON
destroy_session(handle)
poll_events(handle) → AgentEvent[] JSON

# Mob lifecycle
mob_create(definition_json) → mob_id  [async]
mob_spawn(mob_id, specs_json) → result JSON  [async]
mob_wire(mob_id, a, b)  [async]
mob_unwire(mob_id, a, b)  [async]
mob_retire(mob_id, meerkat_id)  [async]
mob_list_members(mob_id) → JSON
mob_send_message(mob_id, meerkat_id, msg)  [async]
mob_events(mob_id, after_cursor, limit) → JSON
mob_status(mob_id) → JSON
mob_list() → JSON
mob_lifecycle(mob_id, action)  [async]
mob_run_flow(mob_id, flow_id, params_json) → run_id  [async]
mob_flow_status(mob_id, run_id) → JSON
mob_cancel_flow(mob_id, run_id)  [async]

# Comms
comms_peers(session_id) → JSON
comms_send(session_id, params_json) → JSON  [async]

# Inspection
inspect_mobpack(bytes) → JSON
```

**Build:**
```bash
RUSTFLAGS='--cfg getrandom_backend="wasm_js"' wasm-pack build meerkat-web-runtime --target web --out-dir <dir>
# Note: --out-dir is relative to crate root (meerkat-web-runtime/), not workspace root
```

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
rkat "What is Rust?"                     # "run" is the default subcommand
rkat run "What is Rust?"                 # equivalent explicit form
rkat --realm team-alpha run "Create a todo app" --enable-builtins --enable-shell --stream -v
rkat resume last "keep going"             # resume most recent session
rkat resume 019c8b99 "continue"          # resume by short prefix
rkat continue "next step"                # shortcut for resume last
rkat --realm team-alpha resume sid_abc123 "Now add error handling"
# Batch context: pipe finite content as context
cat document.txt | rkat run "Summarize this document"
git diff | rkat run "Review these changes" --enable-builtins
# Chained pipes: each rkat reads stdin, writes response to stdout
cat data.csv | rkat run "Extract entities" | rkat run "Write a story about them"
# Live streaming: --host --stdin reads stdin line-by-line as events
tail -f app.log | rkat run --host --stdin "Monitor and alert on anomalies"
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

**Skill introspection** surfaces are available on all surfaces:

- CLI: `rkat skills list [--json]`, `rkat skills inspect <id> [--source <name>] [--json]`
- RPC: `skills/list`, `skills/inspect`
- REST: `GET /skills`, `GET /skills/{id}`
- MCP: `meerkat_skills` tool (`action: "list"` / `"inspect"`)
- Rust SDK: `SkillRuntime::list_all_with_provenance()`, `SkillRuntime::load_from_source()`

Introspection returns both active and shadowed skills with their source provenance, enabling debugging of skill resolution order.

### Hooks

Hook config is realm-aware, with compatibility layering from user/project hook files when available.

### Sub-agents

Sub-agents inherit realm context. With comms enabled, parent/child inproc communication is namespace-scoped by realm.

### Inter-agent comms

Comms supports `inproc`, TCP, and UDS. Inproc registry is namespace-segmented; Meerkat uses realm namespace for isolation.

**Silent comms intents**: `AgentBuildConfig.silent_comms_intents` configures request intents that are injected into session context without triggering an LLM turn. Mob meerkats default to `["mob.peer_added", "mob.peer_retired"]`.

### Tool scoping

Tool visibility can change during a session without restarting the agent. All changes are staged then atomically applied at the turn boundary.

- **External filters** — allow-list or deny-list staged via `ToolScopeHandle`, applied at `CallingLlm` boundary. Persisted in session metadata (`tool_scope_external_filter`).
- **Per-turn overlay** — `TurnToolOverlay` on `StartTurnRequest.flow_tool_overlay`. Ephemeral, used by mob flow steps to restrict tools per step.
- **Live MCP mutation** — `mcp/add`, `mcp/remove`, `mcp/reload` stage server changes on the `McpRouter`. Applied at next turn boundary. Removals drain in-flight calls before finalizing.
- **Composition** — most-restrictive wins (allow-lists intersect, deny-lists union, deny beats allow).
- **Agent awareness** — `ToolConfigChanged` event emitted + `[SYSTEM NOTICE]` injected into conversation on any change.

Surface availability:

| Surface | Live MCP | Tool filter | Status |
|---------|----------|-------------|--------|
| JSON-RPC | `mcp/add`, `mcp/remove`, `mcp/reload` | Via session runtime | Fully wired |
| REST | `POST /sessions/{id}/mcp/*` | — | Routes registered (placeholder) |
| CLI | `rkat mcp add/remove --session --live-server-url` | — | Delegates to REST |

### Memory

Semantic memory (`memory_search`) and compaction integrate through the same session/runtime pipeline.

## Reference

For complete method signatures and multi-surface examples, load:
`references/api_reference.md`
