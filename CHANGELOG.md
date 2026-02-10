# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

#### Contracts and Capabilities
- `meerkat-contracts` crate: single source of truth for all wire-facing types, capability model, error contracts, and schema emission
- `CapabilityId` enum with distributed `inventory`-based registration across feature-gated crates
- `CapabilityStatus` (Available, DisabledByPolicy, NotCompiled, NotSupportedByProtocol) for runtime status
- `WireError` canonical error envelope with `ErrorCode` projections to JSON-RPC codes, HTTP status, and CLI exit codes
- `ContractVersion` with semver compatibility checking (currently 0.1.0)
- Composable request fragments: `CoreCreateParams`, `StructuredOutputParams`, `CommsParams`, `HookParams`, `SkillsParams`
- Wire response types: `WireUsage`, `WireRunResult`, `WireEvent`, `WireSessionInfo`, `WireSessionSummary`
- Feature-gated `JsonSchema` derives on all wire types
- `emit-schemas` binary for deterministic schema artifact generation (`artifacts/schemas/`)
- `capabilities/get` endpoint on all four surfaces (CLI, REST, MCP Server, JSON-RPC)

#### Skills System
- `meerkat-skills` crate with skill sources (filesystem, embedded, in-memory, composite), parser, resolver, renderer, and engine
- Core skill contracts in `meerkat-core/src/skills/`: `SkillId`, `SkillScope`, `SkillDescriptor`, `SkillDocument`, `SkillError`, `SkillSource` and `SkillEngine` traits
- 8 embedded skills: `task-workflow`, `shell-patterns`, `sub-agent-orchestration`, `multi-agent-comms`, `mcp-server-setup`, `hook-authoring`, `memory-retrieval`, `session-management`
- Skill inventory section injected into system prompt via `extra_sections` slot
- Per-turn skill injection via `<skill>` tagged blocks prepended to user messages
- `SkillsResolved` and `SkillResolutionFailed` agent events
- Filesystem skill sources: `.rkat/skills/` (project) and `~/.rkat/skills/` (user)

#### Python and TypeScript SDKs
- SDK codegen pipeline (`tools/sdk-codegen/`) reading from `artifacts/schemas/`
- Python SDK (`sdks/python/`): async MeerkatClient with subprocess lifecycle, capability gating, version checks
- TypeScript SDK (`sdks/typescript/`): MeerkatClient with subprocess lifecycle, capability gating, version checks
- Generated types committed (Python: dataclasses, TypeScript: interfaces)
- SDK error types: `MeerkatError`, `CapabilityUnavailableError`, `SessionNotFoundError`, `SkillNotFoundError`
- Python conformance tests (8 type/error tests)

#### SDK Builder
- Builder tool (`tools/sdk-builder/build.py`): resolves features, builds runtime, emits schemas, runs codegen, emits bundle manifest
- Profile presets: `profiles/minimal.toml`, `profiles/standard.toml`, `profiles/full.toml`
- Bundle manifest with source commit, features, contract version, hashes, timestamp

#### Hooks System
- `meerkat-hooks` crate with `DefaultHookEngine`
- 3 hook runtimes: in-process (Rust handlers), command (stdin/stdout JSON), HTTP (remote endpoints)
- 8 hook points: `run_started`, `run_completed`, `run_failed`, `pre_llm_request`, `post_llm_response`, `pre_tool_execution`, `post_tool_execution`, `turn_boundary`
- Guardrail semantics: first deny short-circuits, deny always wins over allow
- Patch semantics: foreground patches applied in `(priority ASC, registration_index ASC)` order
- Background hooks with observe-only pre-hooks and `HookPatchEnvelope` post-hooks
- Failure policies: observe defaults to fail-open, guardrail/rewrite default to fail-closed
- Per-run hook overrides via `HookRunOverrides` (add entries, disable hooks)

#### Sub-Agents
- `agent_spawn` and `agent_fork` tools for parallel sub-agent work
- `agent_status`, `agent_cancel`, `agent_list` management tools
- `SubAgentManager` with concurrency limits, nesting depth control, and budget allocation
- `ContextStrategy` for spawn context: `FullHistory`, `LastTurns(n)`, `Summary`, `Custom`
- `ToolAccessPolicy`: `Inherit`, `AllowList`, `DenyList` for sub-agent tool filtering
- `ForkBudgetPolicy`: `EqualSplit`, `Proportional`, `Fixed` for budget allocation
- Model allowlists per provider for sub-agent spawns

#### Comms (Inter-Agent Communication)
- `meerkat-comms` crate with `Router`, `Inbox`, `InprocRegistry`
- 3 transport backends: Unix Domain Sockets (UDS), TCP, in-process
- `Keypair`/`PubKey`/`Signature` identity system with Ed25519
- `TrustedPeers` trust model with peer verification
- `Envelope` wire format with `MessageKind` variants: `Message`, `Request`, `Response`, `Ack`
- Comms tools: `comms_send`, `comms_request`, `comms_response`, `comms_list_peers`
- Host mode for long-running agents that process comms messages

#### Memory and Compaction
- `meerkat-memory` crate with `HnswMemoryStore` (hnsw_rs + redb)
- `SimpleMemoryStore` for testing
- `MemoryStore` trait in meerkat-core: `index`, `search`, similarity scoring
- `memory_search` builtin tool for agent access to semantic memory
- Memory indexing of compaction discards wired into agent loop
- `DefaultCompactor` in meerkat-session: auto-compact at token threshold, LLM summary, history rebuild
- `CompactionConfig` for threshold tuning

#### Structured Output
- `OutputSchema` type with `MeerkatSchema`, name, strict mode, compat, and format options
- Schema validation and retry logic for structured output
- `SchemaWarning` for compilation issues
- Provider-specific schema adaptation (Anthropic, OpenAI, Gemini)

#### Session Management
- `SessionService` trait in meerkat-core: create, turn, interrupt, read, list, archive
- `EphemeralSessionService` (in-memory) and `PersistentSessionService` (redb-backed)
- `RedbEventStore` append-only event log
- `SessionProjector` materializing `.rkat/sessions/` files from events
- `RedbSessionStore` for session persistence
- All four surfaces (CLI, REST, MCP Server, JSON-RPC) route through `SessionService`

#### JSON-RPC Server
- `meerkat-rpc` crate with JSON-RPC 2.0 over JSONL stdin/stdout
- `SessionRuntime`: stateful agent manager with dedicated tokio tasks per session
- Methods: `initialize`, `session/create`, `session/list`, `session/read`, `session/archive`, `turn/start`, `turn/interrupt`, `config/get`, `config/set`, `config/patch`
- `session/event` notifications with `AgentEvent` payload during turns

#### Builtin Tools
- Task management: `task_create`, `task_update`, `task_get`, `task_list`
- Shell execution: `shell` (Nushell backend), `shell_jobs`, `shell_job_status`, `shell_job_cancel`
- Utility: `wait`, `datetime`
- Three-tier tool policy: `ToolPolicyLayer` soft policies, `EnforcedToolPolicy` hard constraints, per-tool `default_enabled()`

#### MCP Server Capabilities
- `meerkat-mcp-server` crate exposing `meerkat_run` and `meerkat_resume` as MCP tools
- `McpRouterAdapter` relocated from CLI to `meerkat-mcp` for all surfaces

#### Build Profiles
- Profile presets for controlling feature composition: `profiles/minimal.toml`, `profiles/standard.toml`, `profiles/full.toml`
- Profiles drive SDK builder feature resolution and bundle manifests

#### E2E Tests
- 21-scenario E2E smoke test suite across 5 surfaces (CLI, REST, MCP Server, RPC, SDK)
- Integration-real tests for process spawning and live APIs
- Fast test suite gating for CI (unit + integration-fast, skipping doctests)
- Kitchen-sink compound RPC test replacing mock-only coverage

#### Prompt Assembly
- Unified `assemble_system_prompt` with documented precedence: per-request override > config file > config inline > default + AGENTS.md
- `extra_sections` slot for skill inventory injection
- Config fields `agent.system_prompt`, `agent.system_prompt_file`, `agent.tool_instructions` fully wired

### Changed
- Project renamed from "raik" to "Meerkat" with CLI binary `rkat`
- `AgentFactory::build_agent()` is now the centralized agent construction pipeline for all surfaces
- `FactoryAgentBuilder` bridges `AgentFactory` into `SessionAgentBuilder` trait
- All wire types consolidated into `meerkat-contracts` (removed per-surface duplicates)
- Error handling unified via `WireError` with protocol-specific projections
- Helper functions deduplicated: `resolve_host_mode()` to meerkat-comms, `resolve_store_path()` to meerkat-store, `spawn_event_forwarder()` to facade
- OpenAI and Gemini added to default CLI features
- Test infrastructure stabilized: fast test target isolation, real E2E gating, pre-commit hook fixes for bin-only crates

### Changed - Feature defaults
- `meerkat-tools`: comms, mcp, and sub-agents are now optional features (default: on)
  - `--no-default-features` builds tools with zero optional deps
  - Features: `comms`, `mcp`, `sub-agents`
- `meerkat` facade: comms, mcp, and sub-agents are now optional features (default: on)
  - Features: `comms`, `mcp`, `sub-agents`
- `meerkat-rpc`: minimal by default -- no comms/mcp/sub-agents unless explicitly enabled
- `meerkat-rest`: comms is opt-in (default: on), no comms code when disabled
- `meerkat-mcp-server`: comms is opt-in (default: on), no comms code when disabled
- `meerkat-cli`: comms and mcp are opt-in (default: on), all inline code cfg-gated
- `agent_spawn` tool: `host_mode` field removed from schema when comms feature is off

### Fixed
- Anthropic streaming: emit `ToolCallComplete` on `content_block_stop`
- SDK E2E tests: session list uses `session_id` not `id`
- Python SDK async issues and TypeScript SDK brought to feature parity
- `active_skill_ids` now collects from all skill sources (not just embedded)
- SDK builder memory-store feature resolution
- SDK builder feature forwarding and dead `usage_instructions` removal
- `CapabilityStatus` parsing in SDKs and `contract_version` field inclusion
- RPC `session/create` expanded to full `AgentBuildConfig` parity
- Provider schema lowering moved from core to adapters, removing provider leakage
- `thought_signature` removed from generic `ToolCall`/`ToolResult` (provider-specific only)
- Config-driven sub-agent model policy with fail-closed validation
- Sub-agents, comms, and memory enabled through RPC/SDK surfaces

### Removed
- Dead files in meerkat-core: `comms_runtime.rs`, `comms_bootstrap.rs`, `comms_config.rs`, `agent/comms.rs`
- Duplicate `LlmClientAdapter`/`DynLlmClientAdapter` in meerkat-tools (uses canonical from meerkat-client)
- Per-surface wire type definitions (replaced by `meerkat-contracts`)
- Duplicated helper functions across surface crates

## [0.1.0] - Unreleased

Initial development release.

[Unreleased]: https://github.com/lukacf/meerkat/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/lukacf/meerkat/releases/tag/v0.1.0
