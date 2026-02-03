# Implementation Plan: Ordered Transcripts

**Spec:** `docs/spec-ordered-transcripts.md` (v2.15)
**Method:** Light RCT + TDD
**Execution:** Opus swarm teammates

---

## Phase 0: API Verification (Gate 0)

**Goal:** Verify all spec claims against real provider APIs before writing any production code.

### 0.1 Anthropic Verification
- [ ] Confirm `interleaved-thinking-2025-05-14` beta header enables thinking blocks
- [ ] Confirm thinking blocks have `signature` field (required for replay)
- [ ] Confirm `signature_delta` arrives as separate SSE event before `content_block_stop`
- [ ] Confirm thinking → text → tool_use block ordering
- [ ] Confirm tool_use blocks have `id` (not signature)

### 0.2 OpenAI Verification (Responses API)
- [ ] Confirm `/v1/responses` endpoint exists and accepts our schema
- [ ] Confirm `type: "reasoning"` items in output with `summary` array
- [ ] Confirm `summary` contains `[{"type": "summary_text", "text": "..."}]` structure
- [ ] Confirm `encrypted_content` field when `include: ["reasoning.encrypted_content"]`
- [ ] Confirm `type: "function_call"` items have `call_id`, `name`, `arguments` (JSON string)
- [ ] Confirm `type: "message"` content is array of parts with `type: "output_text"`

### 0.3 Gemini Verification
- [ ] Confirm `thoughtSignature` on functionCall parts (already verified via scripts)
- [ ] Confirm `thoughtSignature` on text parts for non-tool turns
- [ ] Confirm NO signature needed on functionResponse (400 if only there)
- [ ] Confirm parallel calls: only FIRST has signature
- [ ] Confirm Gemini 3 preview models require signatures (400 without)

### 0.4 Verification Deliverables
- `scripts/verify_anthropic_thinking.py` - Live API test
- `scripts/verify_openai_responses.py` - Live API test
- `scripts/verify_gemini_signatures.py` - Live API test (extend existing)

**Gate 0 Pass Criteria:** All verification scripts pass with real API calls.

---

## Phase 1: Core Types (TDD)

**Goal:** Implement core types with round-trip tests.

### 1.1 Types
- [ ] `ProviderMeta` enum (Anthropic/Gemini/OpenAi variants)
- [ ] `AssistantBlock` enum (Text/Reasoning/ToolUse)
- [ ] `AssistantMessage` with `blocks: Vec<AssistantBlock>`
- [ ] `ToolCallView<'a>` and `ToolCallIter<'a>`
- [ ] `LlmResponse` separating content from usage

### 1.2 Tests (Write First)
- [ ] Round-trip: `AssistantBlock` → JSON → `AssistantBlock`
- [ ] Round-trip: `ProviderMeta` variants serialize correctly
- [ ] `ToolCallView::parse_args<T>()` works
- [ ] `AssistantMessage::tool_calls()` iterator
- [ ] `impl Display for AssistantMessage` (text only)

---

## Phase 2: Streaming Layer (TDD)

**Goal:** BlockAssembler and LlmEvent updates.

### 2.1 Components
- [ ] `StreamAssemblyError` enum
- [ ] `BlockKey` newtype
- [ ] `BlockSlot` enum
- [ ] `BlockAssembler` with Slab
- [ ] Updated `LlmEvent` with reasoning variants

### 2.2 Tests (Write First)
- [ ] Assembler: text deltas coalesce
- [ ] Assembler: reasoning start → delta → complete
- [ ] Assembler: tool call start → delta → complete
- [ ] Assembler: block ordering preserved
- [ ] Assembler: error cases (orphaned deltas, duplicates)

---

## Phase 3: Provider Adapters (TDD)

**Goal:** Update each provider's streaming and request building.

### 3.1 Anthropic
- [ ] Update `AnthropicContentBlock` struct for thinking
- [ ] Update `AnthropicDelta` struct for thinking_delta, signature_delta
- [ ] Streaming handler uses BlockAssembler
- [ ] Request builder renders blocks with signatures

### 3.2 OpenAI (Responses API Migration)
- [ ] New endpoint `/v1/responses`
- [ ] Request format: `input` array of items
- [ ] Response parsing: reasoning items, function_call items
- [ ] `include: ["reasoning.encrypted_content"]`

### 3.3 Gemini
- [ ] Request builder: signature on functionCall and text parts
- [ ] NO signature on functionResponse
- [ ] Parse thoughtSignature from response

### 3.4 Tests (Write First)
- [ ] Anthropic: blocks → request JSON (thinking with signature)
- [ ] Anthropic: SSE events → blocks (mock stream)
- [ ] OpenAI: blocks → Responses API input format
- [ ] OpenAI: Responses API output → blocks
- [ ] Gemini: blocks → request JSON (signature placement)

---

## Phase 4: Integration (TDD)

**Goal:** Wire everything together.

### 4.1 Components
- [ ] Update `AgentToolDispatcher::dispatch()` to take `ToolCallView<'_>`
- [ ] Add `Session.usage` and `record_usage()`
- [ ] Add model validation in `AgentBuilder::build()`
- [ ] Update agent loop checkpointing

### 4.2 Tests (Write First)
- [ ] Dispatcher receives `ToolCallView` correctly
- [ ] Session usage accumulates
- [ ] Model validation rejects unsupported models
- [ ] Checkpoint saves at correct points

---

## Phase 5: E2E Tests

**Goal:** Full round-trip with real providers.

- [ ] Anthropic: run → thinking blocks → resume → replay
- [ ] OpenAI: run → reasoning items → resume → replay
- [ ] Gemini: run → tool calls with signature → resume → replay
- [ ] Multi-step tool loop preserves all blocks

---

## Swarm Structure

**Lead (me):** Orchestration, code review, integration
**Teammates (opus agents):**
- `api-verifier`: Phase 0 verification scripts
- `types-impl`: Phase 1 core types
- `streaming-impl`: Phase 2 streaming layer
- `provider-impl`: Phase 3 provider adapters (may split per provider)
- `integration-impl`: Phase 4 integration

Teammates work in parallel where dependencies allow.
