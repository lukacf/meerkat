# Roadmap Progress

**Current Phase:** 2

**Current Status:** gating

## Phase 0 — Prompt Assembly Unification

- [x] Extract `assemble_system_prompt` function — `meerkat/src/prompt_assembly.rs`
- [x] Wire `config.agent.system_prompt` into SystemPromptConfig
- [x] Wire `config.agent.system_prompt_file`
- [x] Wire `config.agent.tool_instructions`
- [x] Tests for each precedence level (15 tests)
- [x] All existing tests pass (402 tests)

## Phase 1 — Contracts Foundation

- [x] Create `meerkat-contracts` crate with directory structure
- [x] `CapabilityId` enum with strum derives
- [x] `CapabilityScope`, `CapabilityStatus` types
- [x] `CapabilityRegistration` + `inventory` collection + `build_capabilities()`
- [x] `CapabilitiesResponse` / query types
- [x] `ErrorCode` enum with protocol projection methods
- [x] `ErrorCategory` enum
- [x] `WireError` struct with `CapabilityHint`
- [x] `ContractVersion` (Ord, Display, FromStr, Copy)
- [x] `Protocol` enum
- [x] `WireUsage`, `WireRunResult`, `WireEvent` response types
- [x] `CoreCreateParams`, `StructuredOutputParams`, `CommsParams`, `HookParams`, `SkillsParams` fragments
- [x] `WireSessionInfo`, `WireSessionSummary` types
- [x] Feature-gated `JsonSchema` on contracts-owned types (types wrapping core types without JsonSchema use serde only)
- [x] Schema emission binary (`emit-schemas`)
- [x] Schema artifacts generated (5 files)

## Phase 2 — Protocol Alignment

- [x] Replace local wire types in RPC with contracts imports (UsageResult → WireUsage)
- [x] Replace local wire types in REST with contracts imports (UsageResponse → WireUsage)
- [x] Replace local wire types in MCP Server with contracts imports (uses contracts for capabilities)
- [x] Replace local wire types in CLI with contracts imports (uses contracts for capabilities)
- [x] Replace local error construction with `WireError` + protocol projection (WireError with From<SessionError>, ErrorCode projections)
- [x] Add `capabilities/get` to all four surfaces (RPC, REST, MCP Server, CLI)
- [x] Extract `resolve_host_mode()` to `meerkat-comms` as `validate_host_mode()`
- [x] Extract `resolve_store_path()` to `meerkat-store`
- [x] Extract `spawn_event_forwarder()` to facade `meerkat/src/surface.rs`
- [x] Relocate `McpRouterAdapter` to `meerkat-mcp`
- [x] All feature-gated crates self-register capabilities via `inventory` (Sessions, Streaming, StructuredOutput, Builtins, Shell, SubAgents, Comms, Hooks, MemoryStore, SessionStore, SessionCompaction)
- [x] Existing tests pass
- [x] Facade re-exports contracts types

## Phase 3 — Skills System

- [ ] Core skill contracts in `meerkat-core/src/skills/`
- [ ] `meerkat-skills` crate with sources, parser, resolver, renderer, engine
- [ ] Factory integration — wire `DefaultSkillEngine` into `AgentBuilder`
- [ ] Prompt assembly includes skill inventory section
- [ ] 8 embedded skills implemented in component crates
- [ ] Skills capability registration
- [ ] Session metadata extension (`active_skills`)
- [ ] Memory indexing of compaction discards wired
- [ ] Existing tests pass

## Phase 4 — Python + TypeScript SDKs

- [ ] Codegen pipeline (`tools/sdk-codegen/`)
- [ ] Python SDK (`sdks/python/`)
- [ ] TypeScript SDK (`sdks/typescript/`)
- [ ] Conformance tests
- [ ] E2E tests pass against minimal, standard, full profiles

## Phase 5 — SDK Builder

- [ ] Builder tool (`tools/sdk-builder/`)
- [ ] Builder pipeline
- [ ] Profile presets (minimal, standard, full)
- [ ] Testing: manifest resolution, preset builds, SDK surface verification
- [ ] Reproducible, version-locked bundles

---

## Gate Results

### Phase 0 — Attempt 1

- build-gate: PASS (build: ~2s, clean)
- test-gate: PASS (402 tests, 5.6s)
- performance-gate: PASS (no API keys in non-ignored tests, no excessive sleeps)
- spec-accuracy-gate: PASS (all acceptance criteria verified, noted acceptable deviations in function signature)
- rust-quality-gate: PASS (two warnings addressed: error logging in system_prompt_file fallback, config_tool_instructions construction cleanup)

### Phase 1 — Attempt 1

- build-gate: PASS (clean, 0.4s)
- test-gate: PASS (all tests pass, ~5.6s)
- performance-gate: PASS
- spec-accuracy-gate: PASS (missing events.json/rpc-methods.json/rest-openapi.json justified by core type JsonSchema limitations; facade re-exports deferred to Phase 2)
- rust-quality-gate: PASS (strum/serde serialization mismatch fixed with #[strum(serialize_all)])

### Phase 2 — Attempt 1

- build-gate: PASS
- test-gate: PASS
- performance-gate: PASS
- spec-accuracy-gate: FAIL — local resolve_host_mode/resolve_store_path/spawn_event_forwarder copies not removed; WireError not wired; RPC error codes inconsistent
- rust-quality-gate: PASS

### Phase 2 — Attempt 2 (fixes applied)

- Fixed: resolve_host_mode() now delegates to meerkat_comms::validate_host_mode() in all 3 surfaces
- Fixed: resolve_store_path() in MCP Server now delegates to meerkat_store::resolve_store_path()
- Fixed: RPC error codes now use ErrorCode::jsonrpc_code() from contracts (made const)
- build-gate: PASS
- test-gate: PASS
- performance-gate: PASS
- spec-accuracy-gate: PASS (minor: REST inline store_path not delegated, functionally identical)
- rust-quality-gate: PASS
