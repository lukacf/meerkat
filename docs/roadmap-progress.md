# Roadmap Progress

**Current Phase:** 0

**Current Status:** gating

## Phase 0 — Prompt Assembly Unification

- [x] Extract `assemble_system_prompt` function — `meerkat/src/prompt_assembly.rs`
- [x] Wire `config.agent.system_prompt` into SystemPromptConfig
- [x] Wire `config.agent.system_prompt_file`
- [x] Wire `config.agent.tool_instructions`
- [x] Tests for each precedence level (15 tests)
- [x] All existing tests pass (402 tests)

## Phase 1 — Contracts Foundation

- [ ] Create `meerkat-contracts` crate with directory structure
- [ ] `CapabilityId` enum with strum derives
- [ ] `CapabilityScope`, `CapabilityStatus` types
- [ ] `CapabilityRegistration` + `inventory` collection + `build_capabilities()`
- [ ] `CapabilitiesResponse` / query types
- [ ] `ErrorCode` enum with protocol projection methods
- [ ] `ErrorCategory` enum
- [ ] `WireError` struct with `CapabilityHint`
- [ ] `ContractVersion` (Ord, Display, FromStr, Copy)
- [ ] `Protocol` enum
- [ ] `WireUsage`, `WireRunResult`, `WireEvent` response types
- [ ] `CoreCreateParams`, `StructuredOutputParams`, `CommsParams`, `HookParams`, `SkillsParams` fragments
- [ ] `WireSessionInfo`, `WireSessionSummary` types
- [ ] Feature-gated `JsonSchema` on all wire types
- [ ] Schema emission binary (`emit-schemas`)
- [ ] Schema artifacts committed

## Phase 2 — Protocol Alignment

- [ ] Replace local wire types in RPC with contracts imports
- [ ] Replace local wire types in REST with contracts imports
- [ ] Replace local wire types in MCP Server with contracts imports
- [ ] Replace local wire types in CLI with contracts imports
- [ ] Replace local error construction with `WireError` + protocol projection
- [ ] Add `capabilities/get` to all four surfaces
- [ ] Extract `resolve_host_mode()` to `meerkat-comms`
- [ ] Extract `resolve_store_path()` to `meerkat-store`
- [ ] Extract `spawn_event_forwarder()` to facade `meerkat/src/surface.rs`
- [ ] Relocate `McpRouterAdapter` to `meerkat-mcp`
- [ ] All feature-gated crates self-register capabilities via `inventory`
- [ ] Existing tests pass

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
