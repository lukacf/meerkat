# Meerkat Implementation Plan (RCT Methodology)

## Overview

This plan follows the RCT (Representation Contract Tests) methodology:
**Representations first. Behavior second. Internals last.**

Key principle: RCT tests use **real API calls** to validate that our normalized types accurately capture what providers actually send.

---

## Implementation Variations

The following variations from this plan were made during implementation:

1. **Test Layout**: Tests are organized per-crate (`meerkat/tests/e2e.rs`, `meerkat/tests/integration.rs`, `meerkat-client/src/*.rs` tests) instead of `tests/rct/`, `tests/integration/`, `tests/e2e/` directories. This follows Rust conventions for in-crate tests.

2. **OpManager**: Operation result injection is handled directly in `Agent::run_loop()` in `meerkat-core/src/agent.rs` rather than through a separate `OpManager` type. The functionality (turn-boundary injection, event ordering) is preserved.

3. **BudgetPool**: CLI uses `BudgetLimits` directly for budget enforcement rather than a full `BudgetPool` allocation system. `BudgetPool` is implemented in `meerkat-core/src/budget.rs` for sub-agent budget sharing but not wired to CLI.

---

## 1. Representation Boundaries

### 1.1 LLM Wire Boundaries

| Boundary | From | To | Risk |
|----------|------|-----|------|
| Anthropic SSE stream | Wire bytes | `LlmEvent` enum | Streaming format, content blocks |
| OpenAI SSE stream | Wire bytes | `LlmEvent` enum | Delta format, function calls |
| Gemini stream | Wire bytes | `LlmEvent` enum | Different function calling format |
| Tool call encoding | Provider JSON | `ToolCall` struct | Schema varies per provider |
| Message format | `Message` struct | Provider JSON | Role names, content structure differ |
| Error responses | Provider HTTP/JSON | `LlmError` enum | Error codes, retry hints |

### 1.2 Public API Boundaries

| Boundary | From | To | Risk |
|----------|------|-----|------|
| AgentEvent stream | Internal events | JSON stream | Public streaming API (§5) |
| RunResult | Internal result | JSON output | CLI/SDK return format (§12) |
| MCP tool schemas | Internal types | JSON Schema | meerkat_run/meerkat_resume inputs (§12) |

### 1.3 Persistence Boundaries

| Boundary | From | To | Risk |
|----------|------|-----|------|
| Session checkpoint | `Session` struct | JSON file | Field evolution, version migrations |
| SessionId | `Uuid` v7 | String | Format stability |
| SessionMeta | Timestamps | ISO8601 | Timezone, precision |
| Config loading | TOML/JSON file | `Config` struct | Validation, defaults, layering |
| ArtifactRef | Internal ref | JSON/file | ID encoding, TTL metadata |

### 1.4 Protocol Boundaries

| Boundary | From | To | Risk |
|----------|------|-----|------|
| MCP messages | JSON-RPC | `McpRequest`/`McpResponse` | Protocol version drift |
| Tool schemas | JSON Schema | `ToolDefinition` | Validation rules |

### 1.5 Operation Boundaries (§18)

| Boundary | From | To | Risk |
|----------|------|-----|------|
| OpEvent | Internal events | Turn injection | Ordering, buffering |
| OperationResult | Op output | Message content | Injection format |
| ContextStrategy | Config | Fork/spawn context | Serialization in tool schemas |
| ForkBudgetPolicy | Config | Budget allocation | Serialization in tool schemas |
| ToolAccessPolicy | Config | Sub-agent permissions | Serialization in tool schemas |

---

## 2. Gate 0: RCT Suite (~25 tests)

### 2.1 Provider Contract Tests (Real API Calls)

**Anthropic (5 tests):**
```
tests/rct/anthropic.rs
├── test_streaming_text_delta_normalization
├── test_tool_call_normalization
├── test_stop_reason_mapping
├── test_usage_mapping
└── test_error_response_mapping
```

**OpenAI (5 tests):**
```
tests/rct/openai.rs
├── test_streaming_text_delta_normalization
├── test_tool_call_normalization
├── test_stop_reason_mapping
├── test_usage_mapping
└── test_error_response_mapping
```

**Gemini (5 tests):**
```
tests/rct/gemini.rs
├── test_streaming_text_delta_normalization
├── test_function_call_normalization  # Different format from Anthropic/OpenAI
├── test_stop_reason_mapping
├── test_usage_mapping
└── test_error_response_mapping
```

**Provider Test Requirements:**
- Use explicit, stable model IDs (not defaults)
- Include retry/backoff with jitter for transient failures
- Assert invariants, not exact token streams
- Environment: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`

### 2.2 Internal Round-Trip Tests (Fixtures)

```
tests/rct/internal.rs
├── test_session_checkpoint_empty
├── test_session_checkpoint_complex     # 50+ messages, tool results
├── test_session_id_encoding            # UUID v7 format
├── test_session_meta_timestamps        # ISO8601 fidelity
├── test_config_layering                # defaults + env + project + CLI
├── test_agent_event_json_schema        # All AgentEvent variants
└── test_run_result_json_schema         # RunResult serialization
```

### 2.3 Protocol Contract Tests (Real MCP Server)

```
tests/rct/mcp.rs
├── test_mcp_initialize_handshake
├── test_mcp_tools_list_schema_parse
└── test_mcp_tools_call_round_trip
```

### 2.4 RCT Assertions Summary

| Category | What's Tested | Real API? |
|----------|--------------|-----------|
| Provider streams | LlmEvent normalization | Yes |
| Provider errors | LlmError mapping | Yes |
| Session persistence | Write → read → equals | No |
| Config | Layering + validation | No |
| Public API | AgentEvent/RunResult JSON | No |
| MCP | Protocol compliance | Yes (test server) |
| Op types | ContextStrategy, ForkBudgetPolicy | No |

---

## 3. Gate 1: E2E Scenarios

### 3.1 Core E2E Flows

```
tests/e2e/
├── simple_chat.rs           # User message → LLM response → done
├── tool_invocation.rs       # LLM requests tool → tool runs → result injected
├── multi_turn.rs            # 5-turn conversation maintains context
├── session_resume.rs        # Checkpoint → restart → continue
├── budget_exhaustion.rs     # Hit token limit → graceful stop
├── parallel_tools.rs        # LLM requests 3 tools → all run → results injected
└── sub_agent_fork.rs        # Parent forks child → child completes → result returned
```

### 3.2 E2E Requirements

- Use real external interface (CLI binary or SDK public API)
- Black-box tests (no internal state inspection)
- Start reliably (fail for expected reasons only)
- Use real LLM providers (not mocks) for true E2E validation

### 3.3 E2E Infrastructure

- Real LLM providers (Anthropic, OpenAI, Gemini)
- Test MCP tool server (simple tools for testing)
- Temp directory for checkpoints
- Budget config for controlled exhaustion tests

---

## 4. Gate 2: Integration Choke Points

### 4.1 Choke Point Matrix

| ID | Choke Point | Components | Required Tests |
|----|-------------|------------|----------------|
| CP-LLM-NORM | Provider stream normalization | LlmClient → LlmEvent | `test_anthropic_normalizes`, `test_openai_normalizes`, `test_gemini_normalizes` |
| CP-LLM-ERROR | Provider error normalization | LlmClient → LlmError | `test_provider_error_classification` |
| CP-TOOL-DISCOVERY | Tool discovery + schema validation | McpRouter → ToolRegistry | `test_tool_discovery_validates_schema` |
| CP-TOOL-DISPATCH | Tool routing + execution | ToolDispatcher → MCP → Result | `test_tool_timeout_enforced`, `test_tool_error_captured` |
| CP-SESSION-TX | Session checkpoint atomicity | Session → Store → Resume | `test_checkpoint_atomic_write`, `test_resume_after_crash` |
| CP-CONFIG-MERGE | Config layering | CLI/env/user/project/defaults | `test_config_precedence`, `test_config_type_coercion` |
| CP-RETRY-POLICY | Retry chain | RetryPolicy → LlmError | `test_retryable_errors_retry`, `test_non_retryable_fail_fast` |
| CP-OP-INJECT | Operation result injection | OpManager → Session | `test_results_injected_at_turn_boundary`, `test_artifact_ref_resolution` |
| CP-EVENT-ORDERING | Event ordering at turn boundaries | OpEvent → AgentEvent | `test_event_ordering_preserved`, `test_backpressure_handled` |
| CP-SUB-AGENT-ACCESS | Tool access policy enforcement | SpawnSpec → ToolAccessPolicy | `test_allow_list_enforced`, `test_deny_list_enforced` |
| CP-STATE-MACHINE | Loop state transitions | LoopState → Side effects | `test_no_invalid_transitions`, `test_cancellation_drains_ops` |
| CP-MCP-WIRE | MCP protocol compliance | McpClient ↔ McpServer | `test_mcp_init_handshake`, `test_mcp_tool_call_roundtrip` |

### 4.2 Integration Test Structure

```
tests/integration/
├── llm_normalization.rs     # CP-LLM-NORM, CP-LLM-ERROR
├── tool_dispatch.rs         # CP-TOOL-DISCOVERY, CP-TOOL-DISPATCH
├── session_persistence.rs   # CP-SESSION-TX
├── config_loading.rs        # CP-CONFIG-MERGE
├── retry_policy.rs          # CP-RETRY-POLICY
├── operation_injection.rs   # CP-OP-INJECT, CP-EVENT-ORDERING
├── sub_agent_access.rs      # CP-SUB-AGENT-ACCESS
├── state_machine.rs         # CP-STATE-MACHINE
└── mcp_protocol.rs          # CP-MCP-WIRE
```

---

## 5. Implementation Phases

### Phase 1: Foundation (Gate 0 Green)

**Goal**: All representation types defined, all RCT tests passing.

1. **Core types crate** (`meerkat-core`)
   - Define all representation types (Message, ToolCall, Session, etc.)
   - Define Op types (OpEvent, OperationResult, ContextStrategy, etc.)
   - Implement Serialize/Deserialize with version fields
   - Write internal RCT tests as types are defined

2. **Provider normalizers** (`meerkat-client`)
   - Anthropic event normalizer + RCT (real API)
   - OpenAI event normalizer + RCT (real API)
   - Gemini event normalizer + RCT (real API)
   - Message format converters per provider

3. **Session persistence** (`meerkat-store`)
   - Checkpoint format with version field
   - SessionStore trait + JSONL implementation
   - RCT for round-trip

### Phase 2: Skeleton (Gates 1-2 Red but Runnable)

**Goal**: E2E and integration tests exist and run (failures expected).

4. **Minimal CLI** (`meerkat-cli`)
   - Basic `rkat run` command
   - JSON output mode
   - Enough for E2E tests to invoke

5. **E2E test harness**
   - Test MCP tool server
   - CLI test runner
   - Write all E2E scenarios (expect red)

6. **Integration test harness**
   - Component test fixtures
   - Write all integration tests (expect red)

### Phase 3: Core Loop (Integration Green)

**Goal**: All integration tests pass.

7. **Tool router** (`meerkat-tools`)
   - ToolRegistry with schema validation
   - ToolDispatcher with timeouts
   - Pass CP-TOOL-DISCOVERY, CP-TOOL-DISPATCH

8. **Config loader** (`meerkat-core`)
   - Layered config (CLI/env/user/project/defaults)
   - Type coercion (string → Duration, etc.)
   - Pass CP-CONFIG-MERGE

9. **Retry policy** (`meerkat-core`)
   - RetryPolicy implementation
   - Integration with LlmError.is_retryable()
   - Pass CP-RETRY-POLICY

10. **Operation manager** (`meerkat-core`)
    - OpManager with event buffering
    - Turn-boundary injection
    - Pass CP-OP-INJECT, CP-EVENT-ORDERING

11. **State machine** (`meerkat-core`)
    - LoopState transitions
    - Side effect coordination
    - Pass CP-STATE-MACHINE

12. **MCP client** (`meerkat-mcp-client`)
    - Connection management
    - Tool discovery
    - Pass CP-MCP-WIRE

13. **Core loop** (`meerkat-core`)
    - Main executor integrating all components
    - Pass remaining integration tests

### Phase 4: E2E Green

**Goal**: All E2E tests pass, system is functional.

14. **Complete CLI** (`meerkat-cli`)
    - Full command set (run, resume, sessions)
    - Event streaming output
    - Pass simple_chat, tool_invocation, multi_turn

15. **Session resume**
    - Checkpoint loading and continuation
    - Pass session_resume E2E

16. **Budget enforcement**
    - BudgetPool implementation
    - Pass budget_exhaustion E2E

17. **Sub-agents**
    - Fork/spawn implementation
    - ToolAccessPolicy enforcement
    - Pass sub_agent_fork E2E, parallel_tools E2E

### Phase 5: Access Patterns

**Goal**: All access patterns working.

18. **MCP server** (`meerkat-mcp-server`)
    - Expose meerkat_run, meerkat_resume tools
    - Protocol compliance

19. **SDK** (`meerkat` crate)
    - Public API surface
    - Builder patterns
    - Documentation

20. **REST API** (optional, `meerkat-rest`)
    - HTTP endpoints
    - OpenAPI spec

---

## 6. Crate Structure

Aligned with DESIGN.md §4:

```
meerkat/
├── Cargo.toml                    # Workspace root
├── DESIGN.md
├── IMPLEMENTATION_PLAN.md
│
├── meerkat-core/                    # Core agent logic
│   ├── src/
│   │   ├── types/                # Message, ToolCall, Session, Op types
│   │   ├── traits/               # LlmClient, SessionStore, ToolProvider
│   │   ├── budget.rs             # BudgetPool
│   │   ├── config.rs             # Config + layering
│   │   ├── retry.rs              # RetryPolicy
│   │   ├── state.rs              # LoopState machine
│   │   ├── loop.rs               # Core executor
│   │   └── lib.rs
│   └── Cargo.toml
│
├── meerkat-client/                  # LLM provider implementations
│   ├── src/
│   │   ├── traits.rs             # LlmClient trait, LlmEvent, LlmError
│   │   ├── anthropic.rs
│   │   ├── openai.rs
│   │   ├── gemini.rs
│   │   └── lib.rs
│   └── Cargo.toml
│
├── meerkat-tools/                   # Tool validation and dispatch
│   ├── src/
│   │   ├── registry.rs           # ToolRegistry + schema validation
│   │   ├── dispatcher.rs         # ToolDispatcher + timeouts
│   │   └── lib.rs
│   └── Cargo.toml
│
├── meerkat-store/                   # Session persistence
│   ├── src/
│   │   ├── traits.rs             # SessionStore trait
│   │   ├── jsonl.rs              # JSONL file store
│   │   ├── memory.rs             # In-memory (tests)
│   │   └── lib.rs
│   └── Cargo.toml
│
├── meerkat-mcp-client/              # MCP client (connect to tool servers)
│   ├── src/
│   │   ├── router.rs
│   │   ├── connection.rs
│   │   ├── discovery.rs
│   │   └── lib.rs
│   └── Cargo.toml
│
├── meerkat-mcp-server/              # MCP server (expose Meerkat as tool)
│   ├── src/
│   │   ├── server.rs
│   │   ├── tools.rs              # meerkat_run, meerkat_resume
│   │   └── lib.rs
│   └── Cargo.toml
│
├── meerkat-cli/                     # CLI binary
│   ├── src/main.rs
│   └── Cargo.toml
│
├── meerkat/                         # SDK facade crate
│   ├── src/lib.rs
│   └── Cargo.toml
│
├── tests/
│   ├── rct/                      # Gate 0: Representation contract tests
│   │   ├── anthropic.rs
│   │   ├── openai.rs
│   │   ├── gemini.rs
│   │   ├── internal.rs
│   │   └── mcp.rs
│   ├── integration/              # Gate 2: Integration choke points
│   │   ├── llm_normalization.rs
│   │   ├── tool_dispatch.rs
│   │   ├── session_persistence.rs
│   │   ├── config_loading.rs
│   │   ├── retry_policy.rs
│   │   ├── operation_injection.rs
│   │   ├── sub_agent_access.rs
│   │   ├── state_machine.rs
│   │   └── mcp_protocol.rs
│   └── e2e/                      # Gate 1: End-to-end scenarios
│       ├── simple_chat.rs
│       ├── tool_invocation.rs
│       ├── multi_turn.rs
│       ├── session_resume.rs
│       ├── budget_exhaustion.rs
│       ├── parallel_tools.rs
│       └── sub_agent_fork.rs
│
└── test-fixtures/
    └── mcp-test-server/          # Simple MCP server for testing
```

---

## 7. Agent Review Board

### Universal Gates (Always Enabled)

- **RCT Guardian**: Blocks if representation boundaries change without RCT updates
- **Integration Sheriff**: Blocks if choke points touched without coverage
- **Spec Auditor**: Blocks if DESIGN.md MUST requirements violated

### Enabled Optional Gates

- **Concurrency/Ordering Gate**: Sub-agents, parallel tools, event ordering
- **Ops/Deployability Gate**: Session migrations, checkpoint versioning

### Spec-Derived Micro-Gates

- **State Machine Gate**: No invalid LoopState transitions
- **Turn Boundary Gate**: All injections happen at turn boundaries only
- **Provider Parity Gate**: All providers normalize to same LlmEvent semantics

---

## 8. Versioning Strategy

Per GPT-5.2 recommendation:

1. **Session checkpoint**: Include `version: 1` field from day one
2. **ArtifactRef**: Include version in metadata
3. **Config**: Version field for migration support
4. **AgentEvent/RunResult**: Stable JSON schema, versioned if breaking changes needed

---

## 9. Model IDs

Stable model IDs for RCT tests:

| Provider | Model ID | Purpose |
|----------|----------|---------|
| Anthropic | `claude-sonnet-4-20250514` | Primary test model |
| OpenAI | `gpt-4o` | Primary test model |
| Gemini | `gemini-2.0-flash` | Primary test model |

Update these when models are deprecated; RCT failures will surface deprecations.

---

## 10. Success Criteria

| Gate | Criteria |
|------|----------|
| Gate 0 | All ~25 RCT tests green (including real API calls) |
| Gate 1 | All 7 E2E scenarios runnable (red OK initially) |
| Gate 2 | All 12 choke points have tests (red OK initially) |
| Gate 3 | Unit tests added as needed for internals |
| MVP | Gates 0-2 all green, CLI works for basic use case |

---

## 11. Open Questions (Resolved)

| Question | Resolution |
|----------|------------|
| Mock LLM fidelity | Use real APIs, not mocks |
| RCT scope for provider tests | Real API calls, assert invariants |
| Checkpoint versioning | Include version field from v0.1 |
| Unknown field preservation | Deferred; no promise in v0.1 |
| AgentEvent vs OpEvent | Keep separate; OpEvent is internal |

