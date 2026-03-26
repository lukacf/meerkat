---
name: meerkat-platform
description: "Comprehensive guide for building applications with the Meerkat agent platform. Covers all surfaces (CLI, REST, RPC, MCP, Python SDK, TypeScript SDK, Rust SDK), configuration, sessions, streaming, skills, hooks, memory, inter-agent communication, and mob orchestration (spawn, fork, helpers, flows). This skill should be used when users ask how to integrate with Meerkat, build agents, configure the runtime, use the SDK, set up multi-agent systems, or work with any Meerkat feature."
---

# Meerkat Platform Guide

Meerkat (`rkat`) is a library-first agent runtime exposed through multiple surfaces. The execution pipeline is shared, state is realm-scoped, and 0.5 semantics are runtime-backed by default.

If the request is about upgrading older integrations or mental models, load:

- `references/migration_0_5.md`

Use that migration guide for:

- `host_mode` -> `keep_alive`
- `--host` -> `--keep-alive`
- runtime-backed ownership vs substrate
- request cancellation / commit semantics
- external-event behavior
- SDK and wire-shape changes

## Realm-first model

`realm_id` is the sharing key across all surfaces.

- Same `realm_id` => shared sessions/config/backend.
- Different `realm_id` => strict isolation.
- Backend (`sqlite`, `redb`, or `jsonl`) is pinned per realm in `realm_manifest.json`.

Default persistent backend:

- New persistent realms default to `sqlite` when sqlite support is compiled.
- `sqlite` is the normal same-realm multi-process backend.
- `redb` remains explicit for single-owner embedded workflows.
- `jsonl` remains explicit for inspectable file-backed persistence.

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
| Web SDK | `@rkat/web` (WASM) | Browser applications |
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
rkat mob respawn <mob_id> <meerkat_id> [--message <msg>]
rkat mob wire <mob_id> <a> <b>
rkat mob unwire <mob_id> <a> <b>
rkat mob turn <mob_id> <meerkat_id> <message>
rkat mob stop|resume|complete <mob_id>
rkat mob events <mob_id> [--member <meerkat_id>]
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

### WASM runtime + Web SDK (embedded deployment target)

The `meerkat-web-runtime` crate is an **embedded deployment target for mobpacks** — the JavaScript equivalent of the Rust SDK. It compiles the real meerkat agent stack to wasm32, routing through the same `AgentFactory::build_agent()` pipeline as all other surfaces.

**`@rkat/web` npm package** (`sdks/web/`): TypeScript wrapper with `MeerkatRuntime`, `Mob`, `Session`, `EventSubscription` classes. Ships with pre-built WASM binary and a Node.js provider proxy.

**Not a protocol server** — unlike RPC/REST, the WASM runtime is deployed INTO a host application (browser). A mobpack defines what it is. The host provides credentials and drives interaction.

**Provider proxy** (`sdks/web/proxy/`): Node.js auth-injecting reverse proxy so API keys stay server-side. The WASM runtime uses per-provider base URLs (`anthropicBaseUrl`, `openaiBaseUrl`, `geminiBaseUrl`) to point at the proxy natively — no fetch override needed.

```bash
ANTHROPIC_API_KEY=sk-... npx @rkat/web proxy --port 3100
```

```typescript
import { MeerkatRuntime } from '@rkat/web';
import * as wasm from '@rkat/web/wasm/meerkat_web_runtime.js';

const runtime = await MeerkatRuntime.init(wasm, {
  anthropicApiKey: 'proxy',
  anthropicBaseUrl: 'http://localhost:3100/anthropic',
});
const mob = await runtime.createMob(definition);
await mob.spawn([{ profile: 'worker', meerkat_id: 'w1' }]);
```

**Architecture:**
- Routes through `EphemeralSessionService<FactoryAgentBuilder>` → `AgentFactory::build_agent()` with override-first resource injection
- `MobMcpState` handles all mob lifecycle operations (same state manager as native MCP mob surface)
- `tokio_with_wasm` provides the async runtime (single-threaded, JS event loop backed)
- `reqwest` uses browser `fetch` on wasm32
- `InprocRegistry` provides peer discovery for comms (all sessions share one process-global registry)
- 10 dependency crates compile for wasm32 (core, client, store, tools, session, hooks, comms, mob, mob-mcp, facade)
- Gemini uses `x-goog-api-key` header (not query param) for auth

**Fully available on wasm32:**
- Agent loop (streaming, retries, error recovery, budget enforcement)
- All three LLM providers (Anthropic, OpenAI, Gemini) via browser fetch
- Session lifecycle (EphemeralSessionService)
- Mob orchestration (MobBuilder, MobActor, FlowEngine, in-memory storage)
- Comms (inproc — InprocRegistry, Ed25519 signing, peer discovery)
- Tool dispatch (task tools, utility builtins like `wait`/`datetime`/`apply_patch`, comms tools, skill tools — no shell)
- Skills (embedded + memory sources from mobpack)
- Hooks (in-process + HTTP — no command hooks)
- Config (in-memory, programmatic)
- Compaction (DefaultCompactor)

**Not available on wasm32 (inherent browser limitations):**
- Filesystem (config loading, AGENTS.md, skill files, session persistence)
- Shell tool, process spawning
- MCP protocol client (rmcp blocked by tokio/mio)
- Network comms (TCP/UDS sockets — inproc only)

**WASM API surface:** See the meerkat-wasm skill (`references/api_surface.md`) for the complete export table. Key additions in 0.4.2: `runtime_version()`, `register_tool_callback()`, `clear_tool_callbacks()`, `create_session_simple()`, per-provider base URLs on all config types.

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
- **Backend selection**: realm-level storage backend (`sqlite`/`redb`/`jsonl`) pinned in `realm_manifest.json`.

Do not conflate the two: mob tool availability is a surface behavior, backend is a realm storage choice.

## Quick start

### CLI

```bash
rkat "What is Rust?"                     # "run" is the default subcommand
rkat run "What is Rust?"                 # equivalent explicit form
rkat --realm team-alpha run "Create a todo app" --enable-builtins --enable-shell --stream -v
# Global flags: --realm, --isolated, --instance, --realm-backend, --state-root, --context-root, --user-config-root
rkat resume last "keep going"             # resume most recent session
rkat resume 019c8b99 "continue"          # resume by short prefix
rkat continue "next step"                # shortcut for resume last
rkat --realm team-alpha resume sid_abc123 "Now add error handling"
# Batch context: pipe finite content as context
cat document.txt | rkat run "Summarize this document"
git diff | rkat run "Review these changes" --enable-builtins
# Chained pipes: each rkat reads stdin, writes response to stdout
cat data.csv | rkat run "Extract entities" | rkat run "Write a story about them"
# Live streaming: --keep-alive --stdin reads stdin line-by-line as events
tail -f app.log | rkat run --keep-alive --stdin "Monitor and alert on anomalies"
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
import { MeerkatClient } from "@rkat/sdk";

const client = new MeerkatClient();
await client.connect({ realmId: "team-alpha" });
const session = await client.createSession("What is Rust?");
await client.close();
```

### Rust SDK

```rust
use meerkat::{
    AgentFactory, Config, CreateSessionRequest, SessionService,
    build_persistent_service, open_realm_persistence_in,
};
use meerkat_core::service::InitialTurnPolicy;
use meerkat_store::RealmBackend;

let config = Config::load().await?;
let realms_root = std::env::current_dir()?.join(".rkat").join("realms");
let (_manifest, persistence) = open_realm_persistence_in(
    &realms_root,
    "team-alpha",
    Some(RealmBackend::Sqlite),
    None,
).await?;
let factory = AgentFactory::new(realms_root.clone())
    .runtime_root(realms_root)
    .builtins(true)
    .shell(true);
let service = build_persistent_service(factory, config, 64, persistence);
let result = service.create_session(CreateSessionRequest {
    model: "claude-sonnet-4-5".into(),
    prompt: "What is Rust?".into(),
    system_prompt: None,
    max_tokens: None,
    event_tx: None,
    skill_references: None,
    initial_turn: InitialTurnPolicy::RunImmediately,
    build: None,
    labels: None,
}).await?;
```

For detailed surface schemas and examples, also load:

- `references/api_reference.md`
- `references/mobs.md`
- `references/migration_0_5.md`

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
meerkat = "0.4"

# Single provider, minimal
meerkat = { version = "0.4", default-features = false, features = ["anthropic"] }

# Add persistence + memory + comms
meerkat = { version = "0.4", features = [
    "jsonl-store", "session-store", "session-compaction",
    "memory-store-session", "comms", "mcp", "skills"
] }
```

Available features: `anthropic`, `openai`, `gemini`, `all-providers`, `jsonl-store`, `memory-store`, `session-store`, `session-compaction`, `memory-store-session`, `comms`, `mcp`, `skills`.

Prebuilt binaries (`rkat`, `rkat-rpc`, `rkat-rest`, `rkat-mcp`) include everything. Custom binary builds:

```bash
cargo install rkat --no-default-features --features "anthropic,openai,session-store,mcp"
```

Disabled features return typed errors (e.g. `SessionError::PersistenceDisabled`) — no panics.

### Model catalog

The `meerkat-models` crate provides a curated model catalog queryable from all surfaces:

- CLI: `rkat models catalog`
- RPC: `models/catalog`
- REST: `GET /models/catalog`
- MCP: `meerkat_models_catalog`

Returns `ModelsCatalogResponse` with providers, default models, and per-model profiles (capabilities, parameter schemas).

### Mid-session model hot-swap

Model and provider can be changed on a live session without rebuilding the agent:

- **RPC**: `turn/start` with `model`, `provider`, `provider_params` fields. Works on both pending (deferred) and materialized sessions.
- **REST**: `POST /sessions/{id}/messages` with `model`, `provider` fields.
- **MCP**: `meerkat_resume` with `model`, `provider` fields.
- **Rust SDK**: `Agent::replace_client()` for direct library usage.

On materialized sessions, the LLM client is hot-swapped for the remainder of the session. Ephemeral sessions return `SessionError::Unsupported`.

## Core features

### Multimodal content (v0.4.12)

Prompts and tool results support multimodal content (text + images). The `ContentInput` type (`Text(String)` or `Blocks(Vec<ContentBlock>)`) is used for all prompt parameters across surfaces.

**SDK prompt types:**
- Python: `prompt: str | list[dict]` — dicts with `{"type": "text", "text": "..."}` or `{"type": "image", "media_type": "...", "data": "<base64>"}`
- TypeScript: `prompt: string | ContentBlock[]` — `{type: "text", text: "..."}` or `{type: "image", mediaType: "...", data: "<base64>"}`
- Rust: `prompt: ContentInput` — `ContentInput::Text(s)` or `ContentInput::Blocks(vec![...])`; implements `From<&str>` and `From<String>`

**view_image builtin tool:** Reads images from disk (PNG/JPEG/GIF/WebP/SVG), returns base64 `ContentBlock::Image`. Path sandboxed to project root. 5 MB limit. Hidden on non-vision-capable models via `ToolScope` based on `ModelProfile.vision` and `ModelProfile.image_tool_results`.

**Provider capabilities:**
| Provider | `vision` | `image_tool_results` |
|----------|----------|---------------------|
| Anthropic | Yes | Yes |
| OpenAI | Yes | No |
| Gemini | Yes | Yes |

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

### Delegated work

Use mobs for all multi-agent orchestration. Mobs support `MemberLaunchMode::Fork` for forking a member's conversation history (via `Session::fork()` CoW), `spawn_helper()`/`fork_helper()` for one-call convenience, `force_cancel_member()` for cancelling in-flight turns, and `member_status()`/`wait_one()`/`wait_all()`/`collect_completed()` for monitoring.

### Inter-agent comms

Comms supports `inproc`, TCP, and UDS. Inproc registry is namespace-segmented; Meerkat uses realm namespace for isolation.

**Silent comms intents**: `AgentBuildConfig.silent_comms_intents` configures request intents that are injected into session context without triggering an LLM turn. Mob meerkats default to `["mob.peer_added", "mob.peer_retired"]`.

#### Structured output with comms (autonomous agents)

In `autonomous_host` mode, agents run a continuous loop: wake on inbox → process (LLM calls + tool calls including `send`) → produce final text output → sleep. Key architectural points:

- **`output_schema` constrains the agent's final text output**, not tool call arguments. It triggers an extraction turn after the agentic loop completes, calling the LLM with no tools and API-enforced structured output.
- **Comms `send` tool body is free-text** (`Option<String>`). There is no schema enforcement on comms message content — agents communicate naturally.
- **The extraction turn fires after each keep-alive processing cycle.** Each time the runtime comms drain consumes inbox work, the agent processes it, sends replies, and then produces a structured JSON summary of what it did. This summary is API-enforced via `output_schema` on the profile.
- **Use case**: Set `output_schema` on autonomous agent profiles to get structured turn summaries (e.g. `{headline: string, details: string}`) while letting agents communicate freely via `send`. The summaries power compact UI displays; the raw comms messages are available for detailed views.
- **Event stream**: The structured output appears in `RunCompleted` events as a JSON string in the `result` field. Parse it to extract the schema-validated fields.

### Tool scoping

Tool visibility can change during a session without restarting the agent. All changes are staged then atomically applied at the turn boundary.

- **External filters** — allow-list or deny-list staged via `ToolScopeHandle`, applied at `CallingLlm` boundary. Persisted in session metadata (`tool_scope_external_filter`).
- **Per-turn overlay** — `TurnToolOverlay` on `StartTurnRequest.flow_tool_overlay`. Ephemeral, used by mob flow steps to restrict tools per step.
- **Live MCP mutation** — `mcp/add`, `mcp/remove`, `mcp/reload` stage server changes on the `McpRouter`. Applied at next turn boundary. Removals drain in-flight calls before finalizing.
- **Async MCP loading** — At startup, MCP servers connect in parallel in the background. The agent loop polls `poll_external_updates()` at each `CallingLlm` boundary. Tools appear as each server completes its handshake. A `[MCP_PENDING]` system notice is injected while servers are still connecting.
  - Per-server timeout: `connect_timeout_secs` in `.rkat/mcp.toml` (default: 10s)
  - CLI: `--wait-for-mcp` flag blocks before the first turn until all servers finish connecting
  - SDK: `McpRouterAdapter::wait_until_ready(timeout)` provides the same blocking behavior
  - `AgentBuildConfig.wait_for_mcp: bool` field for programmatic surface control
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
