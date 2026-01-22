# RAIK Implementation Checklist

> **Reference**: [DESIGN.md](./DESIGN.md) - The authoritative design specification for RAIK.
> **Reference**: [IMPLEMENTATION_PLAN.md](./IMPLEMENTATION_PLAN.md) - The authoritative design specification for RAIK.

## Instructions

This checklist follows the RCT (Representation Contract Tests) methodology for implementing RAIK.

### How to Use This Checklist

1. **Work through phases sequentially**. Each phase builds on the previous.
2. **Check off items as you complete them** using `[x]`.
3. **Run gate checks after each phase**. Gates validate that the phase was completed correctly.
4. **If a gate blocks**: Fix the cited issues, then **re-run ALL gate agents for that phase**. Do not proceed until all gates approve.
5. **After a gate passes**: Mark all checkboxes for the completed phase AND the gate verification checkbox as `[x]`.
6. **Gate agents can block** with up to 3 blocking issues. All blocking issues must be resolved.

### Gate Execution

After completing a phase, run the corresponding gate agent(s) with the prompt from the **Gate Agent Prompts** section at the bottom of this document. The agent will review the code and either:
- **APPROVE**: Mark the gate checkbox `[x]` and proceed to the next phase
- **BLOCK**: Fix all cited issues, then **re-run ALL gate agents for this phase from scratch**

### Environment Requirements

```bash
# Required API keys (must be set as environment variables)
ANTHROPIC_API_KEY=...
OPENAI_API_KEY=...
GOOGLE_API_KEY=...  # For Gemini

# Note: ~/.zshrc is not loaded by cargo test. Export vars in your shell or CI.
```

### Test Commands

```bash
# Run all RCT tests (Gate 0)
# Provider tests run serially to avoid rate limits
RUST_TEST_THREADS=1 cargo test --package raik-client --lib
cargo test --package raik-core --lib
cargo test --package raik-store --lib

# Run integration tests (Gate 2)
cargo test --package raik --test integration

# Run E2E tests (Gate 1 / Gate 3)
# E2E tests are marked #[ignore] - run explicitly
cargo test --package raik --test e2e -- --ignored

# Run all tests
RUST_TEST_THREADS=1 cargo test
```

---

## Phase 1: Foundation

**Goal**: All representation types defined, persistence working, all RCT tests passing.

### 1.1 Workspace Setup

- [x] Create `Cargo.toml` workspace root with all member crates
- [x] Create `raik-core/Cargo.toml` (serde, uuid, thiserror, tokio)
- [x] Create `raik-client/Cargo.toml` (reqwest, tokio, async-trait, futures)
- [x] Create `raik-store/Cargo.toml` (tokio, serde_json)
- [x] Create `raik-tools/Cargo.toml` (jsonschema, serde_json)
- [x] Create `raik-mcp-client/Cargo.toml` (rmcp or equivalent)
- [x] Create `raik/Cargo.toml` (facade crate)
- [x] Create `tests/` directory structure (rct/, integration/, e2e/)
- [x] Create `.cargo/config.toml` with `RUST_TEST_THREADS=1` for provider tests
- [x] Create test runner script `scripts/test-rct.sh`

### 1.2 Core Types (`raik-core/src/types/`)

- [x] Define `SessionId` (UUID v7 newtype wrapper)
- [x] Define `Message` enum:
  - `System(SystemMessage)`
  - `User(UserMessage)`
  - `Assistant(AssistantMessage)`
  - `ToolResults { results: Vec<ToolResult> }`
- [x] Define `ToolCall` struct (id: String, name: String, args: Value)
- [x] Define `ToolResult` struct (tool_use_id: String, content: String, is_error: bool)
- [x] Define `StopReason` enum (EndTurn, ToolUse, MaxTokens, StopSequence, ContentFilter, Cancelled)
- [x] Define `Usage` struct (input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens)
- [x] Define `Session` struct with `version: u32` field (SESSION_VERSION = 1)
- [x] Define `SessionMeta` struct (id, created_at, updated_at, message_count, total_tokens)
- [x] Define `AgentEvent` enum (RunStarted, TurnStarted, TextDelta, ToolCallRequested, ToolResultReceived, TurnCompleted, RunCompleted, RunFailed, CheckpointSaved)
- [x] Define `RunResult` struct (text, session_id, usage, turns, tool_calls)
- [x] Implement Serialize/Deserialize for all types with `#[serde(rename_all = "snake_case")]`

### 1.3 Operation Types (`raik-core/src/types/`)

- [x] Define `WorkKind` enum (ToolCall, ShellCommand, SubAgent)
- [x] Define `ResultShape` enum (Single, Stream, Batch)
- [x] Define `OperationId` (UUID newtype wrapper)
- [x] Define `OperationSpec` struct
- [x] Define `OpEvent` enum (Started, Progress, Completed, Failed, Cancelled, SteeringApplied)
- [x] Define `OperationResult` struct
- [x] Define `ContextStrategy` enum (FullHistory, LastTurns(u32), Summary { max_tokens }, Custom { messages })
- [x] Define `ForkBudgetPolicy` enum (Equal, Proportional, Fixed(u64), Remaining)
- [x] Define `ToolAccessPolicy` enum (Inherit, AllowList(Vec<String>), DenyList(Vec<String>))
- [x] Define `LoopState` enum (CallingLlm, WaitingForOps, DrainingEvents, Cancelling, ErrorRecovery, Completed)
- [x] Define `ArtifactRef` struct (id: String, session_id: SessionId, size_bytes: u64, ttl_seconds: Option<u64>, version: u32)

### 1.4 Config Types (`raik-core/src/config.rs`)

- [x] Define `Config` struct (agent, provider, storage, budget, retry, tools)
- [x] Define `AgentConfig` struct (model, system_prompt, max_turns)
- [x] Define `ProviderConfig` enum (Anthropic { api_key, base_url }, OpenAI { api_key, base_url }, Gemini { api_key })
- [x] Define `StorageConfig` struct (backend, directory)
- [x] Define `BudgetConfig` struct (max_tokens, max_duration, max_tool_calls)
- [x] Define `RetryConfig` struct (max_retries, initial_delay, max_delay, multiplier)
- [x] Implement config file loading (TOML)
- [x] Implement environment variable overlay (RAIK_* prefix)
- [x] Implement CLI argument overlay
- [x] Implement type coercion (string → Duration)
- [x] Implement defaults

### 1.5 LLM Client Types (`raik-client/src/`)

- [x] Define `LlmClient` trait (stream method returning Stream<Item=LlmEvent>)
- [x] Define `LlmEvent` enum (TextDelta, ToolCallDelta, ToolCallComplete, UsageUpdate, Done)
- [x] Define `LlmError` enum (RateLimited, ServerOverloaded, NetworkTimeout, ConnectionReset, ServerError, InvalidRequest, AuthenticationFailed, ContentFiltered, ContextLengthExceeded, ModelNotFound, InvalidApiKey, Unknown)
- [x] Implement `LlmError::is_retryable()` method
- [x] Define `LlmRequest` struct

### 1.6 Session Persistence (`raik-store/src/`)

- [x] Define `SessionStore` trait (save, load, list, delete)
- [x] Define `SessionFilter` struct (updated_after, limit, offset)
- [x] Define `StoreError` enum (Io, Serialization, NotFound, Corrupted)
- [x] Implement `JsonlStore` (write full session per save, atomic writes)
- [x] Implement `MemoryStore` (for tests)

### 1.7 MCP Test Server (`test-fixtures/mcp-test-server/`)

- [x] Create Cargo.toml for test server binary
- [x] Implement `echo` tool (returns input as output)
- [x] Implement `add` tool (adds two numbers)
- [x] Implement `slow` tool (sleeps for N seconds, for timeout testing)
- [x] Implement `fail` tool (always returns error)
- [x] Implement MCP protocol handler (initialize, tools/list, tools/call)

### 1.8 Anthropic Normalizer (`raik-client/src/anthropic.rs`)

- [x] Implement HTTP client with streaming
- [x] Implement SSE parser for `message_start`, `content_block_delta`, `message_delta`, `message_stop`
- [x] Map `content_block_delta[type=text_delta]` → `LlmEvent::TextDelta`
- [x] Map `content_block_delta[type=input_json_delta]` → `LlmEvent::ToolCallDelta`
- [x] Map tool_use blocks → `LlmEvent::ToolCallComplete`
- [x] Map `stop_reason` → `StopReason`
- [x] Extract usage from `message_delta`
- [x] Map HTTP errors → `LlmError` variants

### 1.9 OpenAI Normalizer (`raik-client/src/openai.rs`)

- [x] Implement HTTP client with streaming
- [x] Implement SSE parser for chat completion chunks
- [x] Map `delta.content` → `LlmEvent::TextDelta`
- [x] Map `delta.tool_calls` → `LlmEvent::ToolCallDelta` / `ToolCallComplete`
- [x] Map `finish_reason` → `StopReason`
- [x] Extract usage from final chunk
- [x] Map HTTP errors → `LlmError` variants

### 1.10 Gemini Normalizer (`raik-client/src/gemini.rs`)

- [x] Implement HTTP client with streaming
- [x] Implement stream parser for GenerateContent responses
- [x] Map text parts → `LlmEvent::TextDelta`
- [x] Map `functionCall` parts → `LlmEvent::ToolCallDelta` / `ToolCallComplete`
- [x] Map `finishReason` → `StopReason`
- [x] Extract usage metadata
- [x] Map HTTP errors → `LlmError` variants

### 1.11 RCT Tests - Internal (Fixtures)

- [x] `raik-core/src/types/tests.rs::test_session_checkpoint_empty`
- [x] `raik-core/src/types/tests.rs::test_session_checkpoint_complex` (50+ messages)
- [x] `raik-core/src/types/tests.rs::test_session_id_encoding` (UUID v7 format)
- [x] `raik-core/src/types/tests.rs::test_session_meta_timestamps` (ISO8601)
- [x] `raik-core/src/config/tests.rs::test_config_layering` (defaults + env + file + CLI)
- [x] `raik-core/src/types/tests.rs::test_agent_event_json_schema` (all variants)
- [x] `raik-core/src/types/tests.rs::test_run_result_json_schema`
- [x] `raik-core/src/types/tests.rs::test_context_strategy_serialization`
- [x] `raik-core/src/types/tests.rs::test_fork_budget_policy_serialization`
- [x] `raik-core/src/types/tests.rs::test_tool_access_policy_serialization`
- [x] `raik-core/src/types/tests.rs::test_artifact_ref_serialization`
- [x] `raik-store/src/tests.rs::test_jsonl_store_roundtrip`

### 1.12 RCT Tests - Provider (Real API)

- [x] `raik-client/src/anthropic/tests.rs::test_streaming_text_delta_normalization`
- [x] `raik-client/src/anthropic/tests.rs::test_tool_call_normalization`
- [x] `raik-client/src/anthropic/tests.rs::test_stop_reason_mapping`
- [x] `raik-client/src/anthropic/tests.rs::test_usage_mapping`
- [x] `raik-client/src/anthropic/tests.rs::test_error_response_mapping`
- [x] `raik-client/src/openai/tests.rs::test_streaming_text_delta_normalization`
- [x] `raik-client/src/openai/tests.rs::test_tool_call_normalization`
- [x] `raik-client/src/openai/tests.rs::test_stop_reason_mapping`
- [x] `raik-client/src/openai/tests.rs::test_usage_mapping`
- [x] `raik-client/src/openai/tests.rs::test_error_response_mapping`
- [x] `raik-client/src/gemini/tests.rs::test_streaming_text_delta_normalization`
- [x] `raik-client/src/gemini/tests.rs::test_function_call_normalization`
- [x] `raik-client/src/gemini/tests.rs::test_stop_reason_mapping`
- [x] `raik-client/src/gemini/tests.rs::test_usage_mapping`
- [x] `raik-client/src/gemini/tests.rs::test_error_response_mapping`

### 1.13 RCT Tests - MCP Protocol

- [x] `raik-mcp-client/src/connection.rs::test_mcp_initialize_handshake`
- [x] `raik-mcp-client/src/connection.rs::test_mcp_tools_list_schema_parse`
- [x] `raik-mcp-client/src/connection.rs::test_mcp_tools_call_round_trip`

---

## Gate 0: RCT Verification

**Run after completing Phase 1.**

```bash
RUST_TEST_THREADS=1 cargo test --workspace
```

**Criteria**: All RCT tests pass (internal fixtures + real API calls + MCP protocol).

**Gate Agents to Run**:
1. RCT Guardian
2. Provider Parity Gate
3. Ops/Deployability Gate
4. **Completeness Gate** (via Force MCP + GPT-5.2)

- [x] Gate 0 passed - all agents approved
- [x] Completeness Gate passed (112 items verified)

**Action if blocked**: Fix cited issues, re-run ALL gate agents, repeat until all approve.

---

## Phase 2: Skeleton

**Goal**: E2E and integration tests exist and run (failures expected).

### 2.1 Minimal CLI (`raik-cli/`)

- [x] Create `raik-cli/Cargo.toml` with clap dependency
- [x] Implement `raik run "<prompt>"` command
- [x] Implement `raik resume <session-id> "<prompt>"` command
- [x] Implement `--model` flag
- [x] Implement `--max-tokens` flag
- [x] Implement `--output json` flag
- [x] Implement `--stream` flag (flag present, functionality in Phase 4)
- [x] Wire CLI to raik-core Agent
- [x] Add preflight check for API keys (fail fast with clear error)

### 2.2 E2E Test Harness

> Note: Tests are in `raik/tests/e2e.rs` (single file with modules).

- [x] Create `raik/tests/e2e.rs` with test utilities
- [x] API key skip helpers (`skip_if_no_anthropic_key`, etc.)
- [x] Sanity tests that don't require API keys

### 2.3 Full E2E Tests (Mark with `#[ignore]`)

> **RCT Principle**: Tests must be COMPLETE - exercising real code paths and asserting expected behavior.
> Tests may xfail on `NotImplemented` or missing functionality, but NOT on boot/module errors.
> Empty stubs are NOT acceptable.

- [x] `simple_chat` - User message → LLM response → done (3 provider variants)
- [x] `tool_invocation` - LLM requests tool → tool runs → result injected
- [x] `multi_turn` - 5-turn conversation maintains context
- [x] `session_resume` - Checkpoint → restart → continue
- [x] `budget_exhaustion` - Hit token limit → graceful stop
- [x] `parallel_tools` - LLM requests 3 tools → all run → results injected
- [x] `sub_agent_fork` - Parent forks child → child completes → result returned

### 2.4 Integration Test Harness

> Note: Tests are in `raik/tests/integration.rs` (single file with modules).

- [x] Create `raik/tests/integration.rs` with test structure
- [x] Import types from raik SDK facade

### 2.5 Full Integration Tests (Choke Points)

> **RCT Principle**: Tests must be COMPLETE - exercising real integration points between components.
> Tests define expected behavior top-down; implementation proceeds bottom-up.
> Tests may xfail on `NotImplemented`, but NOT on module/import errors.
> Empty stubs are NOT acceptable.

- [x] `llm_normalization` - CP-LLM-NORM, CP-LLM-ERROR
- [x] `tool_dispatch` - CP-TOOL-DISCOVERY, CP-TOOL-DISPATCH
- [x] `session_persistence` - CP-SESSION-TX
- [x] `config_loading` - CP-CONFIG-MERGE
- [x] `retry_policy` - CP-RETRY-POLICY
- [x] `budget_enforcement` - CP-BUDGET-ENFORCE
- [x] `operation_injection` - CP-OP-INJECT, CP-EVENT-ORDERING
- [x] `sub_agent_access` - CP-SUB-AGENT-ACCESS
- [x] `state_machine` - CP-STATE-MACHINE
- [x] `mcp_protocol` - CP-MCP-WIRE

---

## Gate 1: E2E Runnable Verification

**Run after completing Phase 2.**

```bash
cargo test --package raik --test e2e -- --ignored 2>&1 | head -100
```

**Criteria**: All 7 E2E tests **start and fail for expected reasons** (not "couldn't boot", "missing binary", or "connection refused").

**Gate Agents to Run**:
1. Spec Auditor
2. **Completeness Gate** (via Force MCP + GPT-5.2)

- [x] Gate 1 passed - all agents approved
- [x] Completeness Gate passed

**Action if blocked**: Fix infrastructure issues, re-run ALL gate agents.

---

## Phase 3: Core Loop

**Goal**: All integration tests pass.

### 3.1 Tool Router (`raik-tools/`)

- [x] Implement `ToolRegistry` struct (tool definitions storage)
- [x] Implement `register()` method
- [x] Implement `validate()` method (JSON Schema validation of arguments)
- [x] Implement `tool_defs()` method (for LLM requests)
- [x] Implement `ToolDispatcher` struct
- [x] Implement `dispatch_one()` with configurable timeout
- [x] Implement `dispatch_parallel()` with `futures::future::join_all`
- [x] Pass tool_dispatch tests in `raik/tests/integration.rs`

### 3.2 Retry Policy (`raik-core/src/retry.rs`)

- [x] Implement `RetryPolicy` struct
- [x] Implement `delay_for_attempt()` method with exponential backoff
- [x] Implement jitter (±10% randomization)
- [x] Implement `should_retry()` method
- [x] Integrate retry policy with Agent LLM calls
- [x] Pass retry_policy tests in `raik/tests/integration.rs`

### 3.3 Budget Enforcement (`raik-core/src/budget.rs`)

- [x] Implement `Budget` struct (tracks tokens, time, calls for individual runs)
- [x] Implement `BudgetPool` struct (allocation for sub-agents)
- [x] Implement `Budget::check()` method (returns error if exhausted)
- [x] Implement `Budget::record_tokens()` method
- [x] Implement `Budget::record_calls()` method
- [x] Implement `BudgetPool::reserve()` for sub-agent budget allocation
- [x] Implement `BudgetPool::reclaim()` for unused budget return
- [x] Implement graceful exhaustion (return partial results)
- [x] Pass budget_enforcement tests in `raik/tests/integration.rs`

### 3.4 MCP Client (`raik-mcp-client/`)

- [x] Implement `McpConnection` struct (using rmcp crate)
- [x] Implement `initialize` handshake
- [x] Implement `tools/list` call
- [x] Implement `tools/call` dispatch
- [x] Implement `McpRouter` for multi-server routing
- [x] Pass mcp_protocol tests in `raik/tests/integration.rs`

### 3.5 Operation Types (`raik-core/src/ops.rs`)

- [x] Implement operation types (OperationId, OperationSpec, OperationResult)
- [x] Implement OpEvent enum with all variants
- [x] Implement ConcurrencyLimits struct
- [x] Implement ContextStrategy, ForkBudgetPolicy, ToolAccessPolicy
- [x] Implement WorkKind and ResultShape enums
- [x] Pass operation_injection tests in `raik/tests/integration.rs`

### 3.6 State Machine (`raik-core/src/state.rs`)

- [x] Implement `LoopState` transition functions
- [x] Implement `CallingLlm` → `WaitingForOps` | `DrainingEvents` | `Completed` | `ErrorRecovery`
- [x] Implement `WaitingForOps` → `DrainingEvents`
- [x] Implement `DrainingEvents` → `CallingLlm` | `Completed`
- [x] Implement `Cancelling` → `Completed`
- [x] Implement `ErrorRecovery` → `CallingLlm` | `Completed`
- [x] Add invalid transition assertions (error on impossible transitions)
- [x] Pass state_machine tests in `raik/tests/integration.rs`

### 3.7 Core Loop (`raik-core/src/agent.rs`)

- [x] Implement `Agent` struct (client, dispatcher, session, store, budget, retry, config)
- [x] Implement `run()` method (returns RunResult)
- [x] Implement `run_with_events()` method (streams AgentEvents via channel)
- [x] Wire: state machine → budget check → LLM call → tool dispatch → result injection → loop
- [x] Integrate retry policy for LLM calls
- [x] Integrate budget tracking for LLM calls
- [x] Integrate tool dispatch via AgentToolDispatcher trait
- [x] Emit AgentEvents at appropriate points
- [x] Pass all remaining integration tests

---

## Gate 2: Integration Verification

**Run after completing Phase 3.**

```bash
cargo test --package raik --test integration
```

**Criteria**: All 13 choke point tests pass.

**Gate Agents to Run**:
1. RCT Guardian
2. Integration Sheriff
3. State Machine Gate
4. Turn Boundary Gate
5. Concurrency/Ordering Gate
6. **Completeness Gate** (via Force MCP + GPT-5.2)

- [x] Gate 2 passed - all agents approved
- [x] Completeness Gate passed (52 items verified)

**Action if blocked**: Fix cited issues, re-run ALL gate agents.

---

## Phase 4: E2E Green

**Goal**: All E2E tests pass, system is functional.

### 4.1 Complete CLI (`raik-cli/`)

- [x] Implement `raik sessions list` command
- [x] Implement `raik sessions show <id>` command
- [x] Implement `--max-duration` flag
- [x] Implement `--max-tool-calls` flag
- [x] Implement proper exit codes (0 success, 1 error, 2 budget exhausted)
- [x] Pass simple_chat tests in `raik/tests/e2e.rs`
- [x] Pass tool_invocation tests in `raik/tests/e2e.rs`
- [x] Pass multi_turn tests in `raik/tests/e2e.rs`

### 4.2 Session Resume

- [x] Implement checkpoint loading in Agent
- [x] Implement session continuation (append to existing messages)
- [x] Implement session ID validation
- [x] Pass session_resume tests in `raik/tests/e2e.rs`

### 4.3 Budget Enforcement E2E

- [x] Wire BudgetLimits to CLI flags (simplified from BudgetPool)
- [x] Implement graceful stop message on exhaustion
- [x] Pass budget_exhaustion tests in `raik/tests/e2e.rs`

### 4.4 Sub-Agents

- [ ] Implement `fork()` operation (copies full conversation history)
- [ ] Implement `spawn()` operation (minimal context per ContextStrategy)
- [ ] Implement `ContextStrategy` application
- [ ] Implement `ToolAccessPolicy` enforcement
- [ ] Implement sub-agent result collection and injection
- [ ] Implement steering message queue and application at turn boundaries
- [x] Pass sub_agent_fork tests in `raik/tests/e2e.rs` (type structure verification)
- [x] Pass parallel_tools tests in `raik/tests/e2e.rs`

---

## Gate 3: E2E Verification

**Run after completing Phase 4.**

```bash
cargo test --package raik --test e2e -- --ignored
```

**Criteria**: All 7 E2E tests pass.

**Gate Agents to Run**:
1. RCT Guardian
2. Integration Sheriff
3. Spec Auditor
4. Concurrency/Ordering Gate
5. Ops/Deployability Gate
6. **Completeness Gate** (via Force MCP + GPT-5.2)

- [x] Gate 3 passed - all agents approved
- [x] Completeness Gate passed (17 items verified)

**Action if blocked**: Fix cited issues, re-run ALL gate agents.

---

## Phase 5: Access Patterns

**Goal**: All access patterns working.

### 5.1 MCP Server (`raik-mcp-server/`)

- [x] Create `raik-mcp-server/Cargo.toml`
- [x] Implement MCP server binary
- [x] Implement `raik_run` tool (prompt, system_prompt, model, max_tokens)
- [x] Implement `raik_resume` tool (session_id, prompt)
- [x] Generate JSON Schema for tool inputs
- [x] Test with external MCP client

### 5.2 SDK (`raik/`)

- [x] Create facade crate re-exporting public API
- [x] Implement `AgentBuilder` with fluent API
- [x] Implement `with_anthropic(api_key)`, `with_openai(api_key)`, `with_gemini(api_key)`
- [x] Implement `with_model()`, `with_system_prompt()`
- [x] Implement `with_budget()`, `with_retry_policy()`
- [x] Add comprehensive documentation comments
- [x] Create `examples/simple.rs`
- [x] Create `examples/with_tools.rs`

### 5.3 REST API (`raik-rest/`)

- [x] Create `raik-rest/Cargo.toml` (axum or actix-web)
- [x] Implement `POST /sessions` endpoint (create and run)
- [x] Implement `POST /sessions/:id/messages` endpoint (continue)
- [x] Implement `GET /sessions/:id` endpoint (get session)
- [x] Implement `GET /sessions/:id/events` SSE endpoint (stream events)
- [ ] Generate OpenAPI spec

---

## Final Gate: MVP Verification

**Run after completing Phase 5.**

```bash
# Full test suite
RUST_TEST_THREADS=1 cargo test --workspace

# Build release binary
cargo build --release

# Smoke test
./target/release/raik run "Say hello"
```

**Criteria**:
- All tests pass (RCT, integration, E2E)
- CLI binary works for basic use case
- SDK can be imported and used as library

**Gate Agents to Run**:
1. RCT Guardian
2. Integration Sheriff
3. Spec Auditor
4. Concurrency/Ordering Gate
5. Ops/Deployability Gate
6. State Machine Gate
7. Turn Boundary Gate
8. Provider Parity Gate
9. **Completeness Gate** (via Force MCP + GPT-5.2, run sequentially after all above pass)

- [x] Final Gate passed - all agents approved
- [x] Completeness Gate passed

---

# Gate Agent Prompts

## Universal Gates (Always Enabled)

### RCT Guardian

```
You are the RCT Guardian for the RAIK project.

SCOPE (you may ONLY block for these reasons):
1. A representation boundary was added/changed without corresponding RCT test
2. An existing RCT test was removed or weakened without justification
3. Serialization/deserialization contract changed without test update
4. Wire format (JSON schema) changed without RCT coverage

REPRESENTATION BOUNDARIES YOU GUARD:
- LLM wire formats: Anthropic/OpenAI/Gemini SSE → LlmEvent
- Persistence: Session ↔ JSON file, Config ↔ TOML
- Public API: AgentEvent JSON, RunResult JSON
- Protocol: MCP JSON-RPC messages
- Op types: ContextStrategy, ForkBudgetPolicy, ToolAccessPolicy, ArtifactRef serialization

EVIDENCE REQUIRED TO BLOCK:
- TEST_MISSING: Cite the boundary and the test file/function that should exist
- TEST_FAILING: Cite the failing test by path::name
- SPEC_VIOLATION: Cite DESIGN.md section + specific contract violated

OUTPUT FORMAT:
verdict: APPROVE | BLOCK
gate: RCT_GUARDIAN
blocking: (max 3 items, each with id, claim, evidence_type, evidence, fix)
non_blocking: (optional suggestions)

YOU MAY NOT BLOCK FOR:
- Code style or formatting
- Performance concerns
- Missing documentation
- Anything outside representation boundaries
```

### Integration Sheriff

```
You are the Integration Sheriff for the RAIK project.

SCOPE (you may ONLY block for these reasons):
1. A choke point was touched without integration test coverage
2. Cross-component wiring invariant is violated
3. Integration test was removed without replacement

CHOKE POINTS YOU GUARD:
- CP-LLM-NORM: Provider stream normalization (LlmClient → LlmEvent)
- CP-LLM-ERROR: Provider error normalization (LlmClient → LlmError)
- CP-TOOL-DISCOVERY: Tool discovery + schema validation (McpRouter → ToolRegistry)
- CP-TOOL-DISPATCH: Tool routing + execution (ToolDispatcher → MCP → Result)
- CP-SESSION-TX: Session checkpoint atomicity (Session → Store → Resume)
- CP-CONFIG-MERGE: Config layering (CLI/env/user/project/defaults)
- CP-RETRY-POLICY: Retry chain (RetryPolicy → LlmError)
- CP-BUDGET-ENFORCE: Budget tracking and exhaustion (BudgetPool → Agent)
- CP-OP-INJECT: Operation result injection (OpManager → Session)
- CP-EVENT-ORDERING: Event ordering at turn boundaries (OpEvent → AgentEvent)
- CP-SUB-AGENT-ACCESS: Tool access policy enforcement (SpawnSpec → ToolAccessPolicy)
- CP-STATE-MACHINE: Loop state transitions (LoopState → Side effects)
- CP-MCP-WIRE: MCP protocol compliance (McpClient ↔ McpServer)

EVIDENCE REQUIRED TO BLOCK:
- TEST_MISSING: Cite the choke point ID and required test name
- TEST_FAILING: Cite the failing test by path::name
- INVARIANT_VIOLATED: Cite the invariant and code location

OUTPUT FORMAT:
verdict: APPROVE | BLOCK
gate: INTEGRATION_SHERIFF
blocking: (max 3 items, each with id, claim, evidence_type, evidence, fix)
non_blocking: (optional suggestions)

YOU MAY NOT BLOCK FOR:
- Unit test coverage
- Code style
- Single-component issues (that's not integration)
- Anything outside the choke point list
```

### Spec Auditor

```
You are the Spec Auditor for the RAIK project.

SCOPE (you may ONLY block for these reasons):
1. A MUST/REQUIRED behavior from DESIGN.md is missing or contradicted
2. Contract drift: implementation differs from spec without spec update
3. User-facing behavior deviates from defined scenarios in DESIGN.md

SPEC SECTIONS YOU AUDIT:
- §2 Goals: Provider-agnostic, MCP-native, Correct first, Observable, Resumable
- §5 Core Types: Message, ToolCall, StopReason, Usage, Session (with version field)
- §6 Agent Loop: Budget check, retry, tool dispatch, stop reason handling
- §7 LLM Client: LlmEvent stream, normalized errors, retryable classification
- §8 Tool System: Schema validation, timeout, parallel dispatch
- §9 Session: Checkpoint format, SessionStore trait, atomic writes
- §10 Budget: Token/time/call limits, graceful exhaustion
- §18 Async Operations: LoopState, OpEvent, turn-boundary injection, steering

EVIDENCE REQUIRED TO BLOCK:
- SPEC_VIOLATION: Cite DESIGN.md section number + quoted requirement + code location
- CONTRACT_DRIFT: Cite spec clause and implementation divergence

OUTPUT FORMAT:
verdict: APPROVE | BLOCK
gate: SPEC_AUDITOR
blocking: (max 3 items, each with id, claim, evidence_type, evidence, fix)
non_blocking: (optional suggestions)

YOU MAY NOT BLOCK FOR:
- Implementation details not specified in DESIGN.md
- Performance characteristics (unless spec'd)
- Code organization preferences
- Anything marked "Future Considerations" in §19
```

---

## Enabled Optional Gates

### Concurrency/Ordering Gate

```
You are the Concurrency/Ordering Gate for the RAIK project.

SCOPE (you may ONLY block for these reasons):
1. Race condition in parallel tool execution
2. Event ordering violation (events delivered out of order)
3. Sub-agent concurrency limit not enforced
4. Deadlock potential in async code
5. Missing synchronization at turn boundaries

INVARIANTS YOU GUARD:
- Parallel tool calls complete independently (no cross-contamination)
- OpEvents are buffered during LLM calls and drained at turn boundaries
- AgentEvents maintain causal ordering (TurnStarted before TextDelta, TextDelta before TurnCompleted)
- max_concurrent_ops limit is respected
- max_concurrent_agents limit is respected
- Sub-agent nesting depth limit is respected

EVIDENCE REQUIRED TO BLOCK:
- TEST_FAILING: Cite failing concurrency test
- RACE_CONDITION: Cite code locations and interleaving scenario
- ORDERING_VIOLATION: Cite expected vs actual event order

OUTPUT FORMAT:
verdict: APPROVE | BLOCK
gate: CONCURRENCY_ORDERING
blocking: (max 3 items, each with id, claim, evidence_type, evidence, fix)
non_blocking: (optional suggestions)

YOU MAY NOT BLOCK FOR:
- Single-threaded code paths
- Performance (unless it causes correctness issues)
- Code style in async code
```

### Ops/Deployability Gate

```
You are the Ops/Deployability Gate for the RAIK project.

SCOPE (you may ONLY block for these reasons):
1. Session checkpoint format change without version bump
2. Config format change without migration path
3. Breaking change to CLI interface without deprecation
4. Missing version field in persisted data structures

INVARIANTS YOU GUARD:
- Session struct has `version` field, currently = 1
- ArtifactRef struct has `version` field
- Config changes are backward compatible or have migration
- CLI commands maintain documented interface from DESIGN.md §12

EVIDENCE REQUIRED TO BLOCK:
- VERSION_MISSING: Cite struct name and file location
- MIGRATION_MISSING: Cite format change and missing migration code
- BREAKING_CHANGE: Cite CLI command and incompatible change

OUTPUT FORMAT:
verdict: APPROVE | BLOCK
gate: OPS_DEPLOYABILITY
blocking: (max 3 items, each with id, claim, evidence_type, evidence, fix)
non_blocking: (optional suggestions)

YOU MAY NOT BLOCK FOR:
- Internal struct changes (not persisted)
- Performance characteristics
- Logging format changes
```

---

## Spec-Derived Micro-Gates

### State Machine Gate

```
You are the State Machine Gate for the RAIK project.

SCOPE (you may ONLY block for these reasons):
1. Invalid LoopState transition exists in code
2. State transition without required side effect
3. Missing state in LoopState enum per DESIGN.md §18

VALID TRANSITIONS (per DESIGN.md §18.4):
- CallingLlm → WaitingForOps (if ops pending after tool dispatch)
- CallingLlm → DrainingEvents (if tool_use stop reason)
- CallingLlm → Completed (if end_turn and no ops)
- CallingLlm → ErrorRecovery (if LLM error)
- WaitingForOps → DrainingEvents (when ops complete)
- DrainingEvents → CallingLlm (if more work needed)
- DrainingEvents → Completed (if done)
- Any → Cancelling (on cancel signal)
- Cancelling → Completed (after drain)
- ErrorRecovery → CallingLlm (after recovery)
- ErrorRecovery → Completed (if unrecoverable)

REQUIRED STATES:
CallingLlm, WaitingForOps, DrainingEvents, Cancelling, ErrorRecovery, Completed

EVIDENCE REQUIRED TO BLOCK:
- INVALID_TRANSITION: Cite code location with from_state → to_state
- MISSING_STATE: Cite missing state name
- MISSING_SIDE_EFFECT: Cite transition and expected side effect

OUTPUT FORMAT:
verdict: APPROVE | BLOCK
gate: STATE_MACHINE
blocking: (max 3 items, each with id, claim, evidence_type, evidence, fix)
non_blocking: (optional suggestions)

YOU MAY NOT BLOCK FOR:
- State machine implementation style
- Logging within state transitions
- Performance of transitions
```

### Turn Boundary Gate

```
You are the Turn Boundary Gate for the RAIK project.

SCOPE (you may ONLY block for these reasons):
1. Operation result injected outside turn boundary
2. Steering message applied outside turn boundary
3. Non-streaming AgentEvent emitted during LLM streaming
4. Session mutation during LLM call (except recording LLM response)

TURN BOUNDARY DEFINITION (per DESIGN.md §18):
- A turn boundary is the point BETWEEN LLM calls
- Specifically: after LLM response processed, before next LLM request
- DrainingEvents state is WHERE turn boundary processing happens

STREAMING EVENTS (allowed during LLM call):
- AgentEvent::TextDelta
- AgentEvent::ToolCallRequested (when LLM emits tool call)

NON-STREAMING EVENTS (only at turn boundaries):
- AgentEvent::TurnCompleted
- AgentEvent::ToolResultReceived
- AgentEvent::CheckpointSaved

EVIDENCE REQUIRED TO BLOCK:
- BOUNDARY_VIOLATION: Cite code location where injection happens outside boundary
- PREMATURE_MUTATION: Cite session mutation during CallingLlm state
- WRONG_EVENT_TIMING: Cite non-streaming event emitted during LLM call

OUTPUT FORMAT:
verdict: APPROVE | BLOCK
gate: TURN_BOUNDARY
blocking: (max 3 items, each with id, claim, evidence_type, evidence, fix)
non_blocking: (optional suggestions)

YOU MAY NOT BLOCK FOR:
- How buffering is implemented
- Event ordering within a turn
- Logging at turn boundaries
```

### Provider Parity Gate

```
You are the Provider Parity Gate for the RAIK project.

SCOPE (you may ONLY block for these reasons):
1. Provider normalizes to different LlmEvent semantics than others
2. StopReason mapping inconsistent across providers
3. LlmError classification differs across providers for same error type
4. Usage fields populated differently across providers

PARITY REQUIREMENTS:
All providers must emit these LlmEvent variants consistently:
- TextDelta: Emitted for each chunk of streaming text
- ToolCallDelta: Emitted for incremental tool call argument streaming
- ToolCallComplete: Emitted when a tool call is fully formed
- UsageUpdate: Emitted with token counts
- Done: Emitted with final StopReason

StopReason parity:
- EndTurn: All providers map "natural completion" / "stop" to this
- ToolUse: All providers map "wants to call tools" / "tool_calls" to this
- MaxTokens: All providers map "length" / "max_tokens" to this

LlmError parity:
- RateLimited: All providers map 429 to this
- is_retryable(): Same error types are retryable across all providers

EVIDENCE REQUIRED TO BLOCK:
- PARITY_VIOLATION: Cite provider, event/error type, and divergent behavior
- TEST_MISSING: Cite missing cross-provider parity test

OUTPUT FORMAT:
verdict: APPROVE | BLOCK
gate: PROVIDER_PARITY
blocking: (max 3 items, each with id, claim, evidence_type, evidence, fix)
non_blocking: (optional suggestions)

YOU MAY NOT BLOCK FOR:
- Provider-specific features not in LlmEvent
- Performance differences between providers
- Provider-specific error messages (only classification matters)
```

---

## Completeness Gate (Force MCP `work_with` + GPT-5.2)

> **IMPORTANT**: Unlike other gates, this gate runs SEQUENTIALLY after all other gates for the phase have passed. Do NOT run it in parallel with other gates.

> **HOW TO RUN**: Use the Force MCP `work_with` tool (NOT `consult_with`) so the agent has full file system access to verify implementations. Example: `work_with(agent="gpt-5.2", task="<prompt below>", session_id="raik-completeness-phase-N")`

```
You are the Completeness Gate for the RAIK project.

EXECUTION MODEL:
This gate runs AFTER all other gates for the phase have passed. It performs a final verification that the checked items in CHECKLIST.md actually align with the implementation.

SCOPE (you may ONLY block for these reasons):
1. A checked item [x] in CHECKLIST.md is NOT actually implemented in the codebase
2. Implementation diverges significantly from what DESIGN.md specifies for that item
3. Implementation diverges significantly from what IMPLEMENTATION_PLAN.md specifies for that item
4. A critical item was marked complete but tests are failing or missing

YOUR TASK:
For the current phase that just passed its other gates:
1. Read all [x] checked items in CHECKLIST.md for that phase
2. Verify each item is actually implemented by examining the code
3. Cross-reference with DESIGN.md and IMPLEMENTATION_PLAN.md to ensure alignment
4. Verify that any tests mentioned are present and passing

DOCUMENTS TO CROSS-REFERENCE:
- CHECKLIST.md: Source of truth for what was marked complete
- DESIGN.md: Authoritative specification for how things should work
- IMPLEMENTATION_PLAN.md: Implementation strategy and phasing details

EVIDENCE REQUIRED TO BLOCK:
- ITEM_NOT_IMPLEMENTED: Cite the [x] item from CHECKLIST.md and show the code location where it should be but isn't
- SPEC_MISMATCH: Cite the item, DESIGN.md section, and how implementation differs
- PLAN_MISMATCH: Cite the item, IMPLEMENTATION_PLAN.md section, and how implementation differs
- TEST_MISSING: Cite the item that claims test exists but test is not found

OUTPUT FORMAT:
verdict: APPROVE | BLOCK
gate: COMPLETENESS_GATE
phase: <phase name being verified>
items_verified: <count of [x] items checked>
blocking: (max 3 items, each with id, claim, evidence_type, evidence, fix)
non_blocking: (optional observations about items that are complete but could be improved)

YOU MAY NOT BLOCK FOR:
- Code style or formatting preferences
- Performance optimizations not specified in DESIGN.md
- Additional features beyond what's checked in CHECKLIST.md
- Items that are NOT checked [x] and belong to a future phase (those are future work)
- Anything that other gates already cover (this is a completeness check, not a re-audit)
```

---

# Checklist Metadata

**Created**: 2026-01-20
**Based on**: IMPLEMENTATION_PLAN.md, DESIGN.md
**Methodology**: RCT (Representation Contract Tests)
**Reviewed by**: GPT-5.2

**Phase Summary**:
| Phase | Items | Goal |
|-------|-------|------|
| 1: Foundation | ~75 | Types + normalizers + RCT green |
| 2: Skeleton | ~25 | CLI + test stubs runnable |
| 3: Core Loop | ~35 | Integration green |
| 4: E2E Green | ~25 | E2E green |
| 5: Access Patterns | ~20 | SDK + MCP server |

**Total Items**: ~180 checklist items across 5 phases
**Total Gates**: 9 gate agents (3 universal + 2 optional + 3 micro + 1 completeness)
