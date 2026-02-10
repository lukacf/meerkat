# Spec: Provider-Agnostic Ordered Transcripts (Thinking + Tools) With Resume Fidelity

**Version:** 2.15
**Date:** 2026-02-03
**Branch:** `fixes/thinking-stuff-continuiuty`
**Breaking Changes:** Yes (no backwards compatibility required)

**v2.15 changes:**
- Fixed OpenAI parsing: `TextDelta` now includes `meta: None` (compiles correctly)
- Added `gpt-5.2-codex` to OpenAI allowlist

**v2.14 changes:**
- Fixed Gemini section 2.3: signatures on `functionCall` AND `text` parts, NEVER on `functionResponse`
- Fixed ToolResult rationale to clarify "never on functionResponse" (not "functionCall only")
- Added `signature_delta` to Anthropic SSE event types list
- Added `meta: Option<ProviderMeta>` to `LlmEvent::TextDelta` for Gemini text-part signatures
- Removed `gpt-5.1-codex-max` from allowlist; added GPT-5.1 to rejected models

**v2.13 changes:**
- Added Section 1.3: Supported Models (Minimum Versions)
- Defined model allowlist: Claude 4.5+, GPT-5.2+, Gemini 3 preview+
- Specified model validation in `AgentBuilder::build()` with `UnsupportedModel` error

**v2.12 changes:**
- Added `signature_delta` handling for Anthropic thinking blocks (signatures can arrive as separate delta)
- Anthropic thinking block rendering now REQUIRES signature (skips block if missing)
- Added `meta: Option<ProviderMeta>` to `AssistantBlock::Text` for Gemini thoughtSignature on text parts
- `on_text_delta` now takes optional `meta` parameter for Gemini
- Gemini request rendering includes thoughtSignature on text parts if present
- Added Anthropic tool_result rendering with ordering requirements documented

**v2.11 changes:**
- `finalize_tool_args` now returns `Err(UnknownToolFinalize)` for missing buffer (was silent "{}" fallback)
- OpenAI function_call parsing skips on invalid/missing args (no panic paths)

**v2.10 changes:**
- Added `BlockAssembler::finalize_tool_args()` → `Result<Box<RawValue>, StreamAssemblyError>`
- Added `StreamAssemblyError::InvalidArgsJson` variant
- Added `id` field to `ProviderMeta::OpenAi` for reasoning item replay (required by API)
- Changed `ProviderMeta::OpenAi::encrypted_content` to `Option<String>`
- Fixed dispatcher trait: `tools() -> Arc<[ToolDef]>` (matches codebase naming)
- Removed all `.unwrap()`/`.expect()` from spec code examples

**v2.9 changes:**
- Added `Session::total_usage()` and `Session::total_tokens()` using session-level usage
- Added `SessionMeta` struct with usage field
- Updated `AgentToolDispatcher::dispatch()` to take `ToolCallView<'_>` (breaking trait change)
- Added agent loop usage example for dispatcher

**v2.8 changes:**
- Added `Session.usage` for cumulative token tracking (persisted with session)
- Added `Session::record_usage()` for agent loop integration
- Added `ToolCallView::parse_args<T>()` defining the RawValue→T parsing boundary
- Clarified which assembler methods are fallible vs infallible in changelog

**v2.7 changes:**
- `BlockAssembler` now uses `Slab<BlockSlot>` instead of `Vec` (stable keys)
- `BlockKey(usize)` newtype prevents mixing up different indices
- Fallible assembler methods return `Result<(), StreamAssemblyError>` (separation of concerns)
  - `on_reasoning_delta`, `on_tool_call_start`, `on_tool_call_delta`, `on_tool_call_complete` → `Result`
  - `on_text_delta`, `on_reasoning_start`, `on_reasoning_complete` → `()` (infallible by design)
- Stream handlers show explicit error handling with policy decisions

**v2.6 changes:**
- `LlmEvent` now uses typed `Option<ProviderMeta>` (not `Option<Value>`)
- `LlmStreamResult::into_response()` returns `LlmResponse` (not `into_message()`)
- OpenAI parsing uses `RawValue::from_string()` for args (no intermediate `Value`)
- All provider adapters construct typed `ProviderMeta` variants directly

---

## 0. Problem Statement

We want core-level capabilities (especially upcoming **context compaction**) to have access to the *full within-turn transcript structure*:

- Interleaved "visible thinking / reasoning summaries / notes"
- Tool call(s) and tool result(s)
- The **exact order** those appeared in the model's output stream

...and we must preserve any provider-required continuity tokens so that **resume** (and multi-step tool loops) can round-trip correctly across Anthropic, OpenAI, and Gemini.

### Current State Problems

1. **AssistantMessage is flat**: `content: String` + `tool_calls: Vec<ToolCall>` loses all ordering and interleaving information.

2. **Thinking blocks are completely lost**:
   - Anthropic streaming handler (`anthropic.rs:381-398`) only handles `text_delta` and `input_json_delta`—thinking blocks are silently dropped.
   - No `LlmEvent` variants exist for reasoning/thinking content.
   - No field in `AssistantMessage` to store thinking.
   - On resume, thinking blocks are not rendered back to Anthropic.

3. **Provider-specific leakage**: `thought_signature: Option<String>` is directly on `ToolCall` and `ToolResult` in core types—Gemini-specific data in provider-agnostic types.

4. **Ordering can be nondeterministic**: `HashMap` is used in streaming aggregation (`adapter.rs:86`, `openai.rs:227`), which can produce nondeterministic tool call ordering.

5. **OpenAI uses Chat Completions API**: The current implementation doesn't support reasoning summaries. **Must migrate to Responses API.**

---

## 1. Requirements

### 1.1 Functional Requirements

1. **Capture ordered assistant transcript blocks** per LLM call:
   - Text blocks
   - Reasoning/thinking blocks (visible, model-emitted)
   - Tool-use blocks (with args and optional metadata)

2. **Preserve provider-required continuity artifacts** in an opaque `meta` field so the next request can replay them correctly.

3. **Resume must work** after a completed run (`rkat resume`).

4. **Core-level compaction** can use stored reasoning notes + tool call sequence without caring about provider syntax.

5. **Deterministic ordering**: Tool calls must appear in the same order they were emitted by the model.

6. **Dispatchable tool calls**: The core loop must be able to extract `(id, name, args)` for tool dispatch without provider coupling.

### 1.2 Non-Goals

- Storing "hidden/raw CoT tokens" that the provider doesn't return.
- Requiring providers to expose the same reasoning representation.
- Changing the agent's basic tool loop semantics.
- Backwards compatibility with V1 sessions (clean break).
- Supporting Chat Completions API for OpenAI (Responses API only going forward).
- Supporting legacy models that lack required features.

### 1.3 Supported Models (Minimum Versions)

This spec requires model features (interleaved thinking, reasoning summaries, thought signatures) that are only available in recent model versions. **Meerkat will reject unsupported models at configuration time.**

| Provider | Minimum Model | Required Features |
|----------|---------------|-------------------|
| Anthropic | `claude-sonnet-4-5`, `claude-opus-4-5`, `claude-opus-4-6` | Interleaved thinking with signatures (`interleaved-thinking-2025-05-14` beta) |
| OpenAI | `gpt-5.2`, `gpt-5.2-pro`, `gpt-5.2-codex` | Responses API with reasoning items and `encrypted_content` |
| Gemini | `gemini-3-pro-preview`, `gemini-3-flash-preview` | `thoughtSignature` on function calls and text parts |

**Rejected models** (will error at startup):
- Anthropic: Claude 3.x, Claude 4.0 (no interleaved thinking support)
- OpenAI: GPT-4o, GPT-4.1, GPT-5.1, o1/o3 series (no Responses API or missing reasoning features)
- Gemini: Gemini 1.x, Gemini 2.x (no thoughtSignature support)

**Implementation**: Add model validation in `AgentBuilder::build()` that checks the configured model against an allowlist. Return `Err(UnsupportedModel { model, reason })` for rejected models.

---

## 2. Provider Reality: What Must Round-Trip

### 2.1 Anthropic (Claude)

**Verified against official documentation (February 2026)**

| Aspect | Details |
|--------|---------|
| Tool calling | Ordered content blocks: `text`, `tool_use`, `tool_result` |
| Tool results | Must follow tool_use in a user message with `tool_result` blocks |
| Thinking blocks | `{ "type": "thinking", "thinking": "...", "signature": "..." }` |
| Thinking signatures | **Required** for continuity—must be preserved and returned unchanged |
| Interleaved thinking | Requires beta header `interleaved-thinking-2025-05-14` for Claude 4 models |
| Block ordering | Typically: `thinking` → `text` → `tool_use` |

**Critical**: Tool_use blocks have `id` for correlation, NOT signatures. Only thinking blocks have signatures.

**SSE event types to handle** (requires `AnthropicEvent` struct updates):
- `content_block_start` with `type: "thinking"`
- `content_block_delta` with `type: "thinking_delta"` (text content)
- `content_block_delta` with `type: "signature_delta"` (signature arrives separately before stop)
- `content_block_stop` for thinking block completion

### 2.2 OpenAI (Responses API)

**Verified against official documentation (February 2026)**

| Aspect | Details |
|--------|---------|
| API | **Responses API only** (`/v1/responses`) — Chat Completions is deprecated for our use |
| Reasoning summaries | Explicit `type: "reasoning"` items in output array |
| Tool calling | Function calls as items with `type: "function_call"`, `call_id`, `name`, `arguments` (JSON string) |
| Tool results | Items with `type: "function_call_output"`, `call_id`, `output` |
| Continuity rule | "Include all intermediate items between last user message and tool output untouched" |
| Encrypted reasoning | `encrypted_content` field can be passed back for stateless replay |

**Responses API item types**:
- `message` - text content
- `reasoning` - reasoning/thinking with optional `summary` array
- `function_call` - tool invocation
- `function_call_output` - tool result

### 2.3 Gemini

**Verified against Gemini 3 API testing (February 2026)**

| Aspect | Details |
|--------|---------|
| Thought signatures | Opaque `thoughtSignature` field for continuity |
| Required for | Gemini 3.x models (**400 error if omitted**) |
| Signature location | On `functionCall` parts AND trailing `text` parts; **NEVER on `functionResponse`** |
| Parallel calls | **Only the FIRST functionCall has a signature** |
| Sequential calls | Each step has its own signature |
| Non-tool turns | Signature may appear on text part (even empty text) for continuity |

**Empirically verified** via `scripts/test_gemini_thought_signature.py`:

```
Test A (sig on functionCall only):     PASS ✓
Test B (sig on functionResponse only): FAIL (400 error)
Test C (sig on BOTH):                  PASS (redundant)
Test D (NO sig anywhere):              FAIL (400 error)
```

**Parallel call test** via `scripts/test_gemini_parallel_calls.py`:
```
Call 1 (get_weather):   HAS signature
Call 2 (get_time):      NO signature
Call 3 (get_population): NO signature

Conclusion: Only FIRST parallel call has signature
```

**Implementation note**: Store whatever signature Gemini returns on each part (functionCall or text). First in a parallel batch will have one, others won't. Signatures NEVER go on functionResponse—replay exactly as stored on the original parts.

---

## 3. Solution Design

### 3.1 Core Types (Breaking Change)

Replace the current flat `AssistantMessage` with ordered blocks. Use zero-copy views for access patterns.

```rust
// meerkat-core/src/types.rs

use serde_json::value::RawValue;
use std::fmt;

/// Provider-specific metadata for replay continuity.
/// Typed enum prevents runtime "is this an object?" errors.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum ProviderMeta {
    /// Anthropic thinking block signature
    Anthropic {
        /// Opaque signature for thinking continuity
        signature: String,
    },
    /// Gemini thought signature for tool calls
    Gemini {
        /// Opaque signature for tool call continuity
        #[serde(rename = "thoughtSignature")]
        thought_signature: String,
    },
    /// OpenAI reasoning item metadata for stateless replay.
    /// Both `id` and `encrypted_content` are required for faithful round-trip.
    OpenAi {
        /// Reasoning item ID (required by OpenAI schema)
        id: String,
        /// Encrypted reasoning tokens for continuity
        #[serde(skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<String>,
    },
}

/// A block of content in an assistant message, preserving order.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "block_type", rename_all = "snake_case")]
pub enum AssistantBlock {
    /// Plain text output.
    /// Note: Gemini may attach thoughtSignature to trailing text parts for continuity.
    Text {
        text: String,
        /// Provider metadata (Gemini thoughtSignature on text parts)
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<ProviderMeta>,
    },

    /// Visible reasoning/thinking notes emitted by the model
    Reasoning {
        /// Human-readable text (may be empty if only encrypted content)
        #[serde(default)]
        text: String,
        /// Provider continuity metadata (typed, not arbitrary JSON)
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<ProviderMeta>,
    },

    /// Tool use request
    ToolUse {
        id: String,
        name: String,
        /// Arguments as raw JSON—only dispatcher parses this
        args: Box<RawValue>,
        /// Provider continuity metadata
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<ProviderMeta>,
    },
}

// Tagging note: Using internally tagged enum for human-readable JSON storage.
// For hot-path internal messaging, consider adjacently tagged or untagged.
// This is a storage format, not a wire protocol for microsecond latency.

/// Borrowed view into a tool call block. Zero allocations.
#[derive(Debug, Clone, Copy)]
pub struct ToolCallView<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub args: &'a RawValue,
}

impl ToolCallView<'_> {
    /// Parse args into a concrete type. Called by dispatcher, not core.
    ///
    /// # Errors
    /// Returns `serde_json::Error` if args don't match expected schema.
    pub fn parse_args<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_str(self.args.get())
    }
}

// PARSING BOUNDARY: Core never parses args. The dispatcher calls parse_args<T>()
// where T is the tool's input struct. Parse errors are tool-level errors, not
// protocol errors—the dispatcher returns ToolResult { is_error: true, content: "..." }.

/// Tool dispatcher trait. BREAKING: now takes ToolCallView instead of ToolCall.
#[async_trait]
pub trait AgentToolDispatcher: Send + Sync {
    /// Dispatch a tool call and return the result.
    ///
    /// The dispatcher is responsible for:
    /// 1. Looking up the tool by `call.name`
    /// 2. Parsing args via `call.parse_args::<T>()`
    /// 3. Executing the tool
    /// 4. Returning ToolResult (including is_error=true for parse failures)
    async fn dispatch(&self, call: ToolCallView<'_>) -> ToolResult;

    /// List available tools for LLM request building.
    /// NOTE: Matches existing codebase naming (ToolDef, not ToolDefinition).
    fn tools(&self) -> Arc<[ToolDef]>;
}

// Agent loop usage:
// for call in assistant_msg.tool_calls() {
//     let result = dispatcher.dispatch(call).await;
//     results.push(result);
// }

/// Iterator over tool calls in an AssistantMessage. Zero allocations.
pub struct ToolCallIter<'a> {
    inner: std::slice::Iter<'a, AssistantBlock>,
}

impl<'a> Iterator for ToolCallIter<'a> {
    type Item = ToolCallView<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.inner.next()? {
                AssistantBlock::ToolUse { id, name, args, .. } => {
                    return Some(ToolCallView { id, name, args });
                }
                _ => continue,
            }
        }
    }
}

/// Assistant message content—no billing metadata polluting the domain model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AssistantMessage {
    /// Ordered sequence of content blocks
    pub blocks: Vec<AssistantBlock>,
    /// How the turn ended
    pub stop_reason: StopReason,
}

// Usage is NOT part of AssistantMessage.
// It's billing metadata returned separately from LLM calls.
// See LlmResponse below.

impl AssistantMessage {
    /// Iterate over tool calls without allocation.
    pub fn tool_calls(&self) -> ToolCallIter<'_> {
        ToolCallIter { inner: self.blocks.iter() }
    }

    /// Check if any tool calls are present.
    pub fn has_tool_calls(&self) -> bool {
        self.blocks.iter().any(|b| matches!(b, AssistantBlock::ToolUse { .. }))
    }

    /// Get tool use block by ID.
    pub fn get_tool_use(&self, id: &str) -> Option<ToolCallView<'_>> {
        self.tool_calls().find(|tc| tc.id == id)
    }

    /// Iterate over text blocks without allocation.
    pub fn text_blocks(&self) -> impl Iterator<Item = &str> {
        self.blocks.iter().filter_map(|b| match b {
            AssistantBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
    }
}

/// Display implementation for text content—zero intermediate allocations.
impl fmt::Display for AssistantMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for block in &self.blocks {
            if let AssistantBlock::Text { text, .. } = block {
                f.write_str(text)?;
            }
        }
        Ok(())
    }
}
```

**Key design decisions**:
- `#[non_exhaustive]` allows future block types without breaking changes
- `ProviderMeta` is a **typed enum**, not `Option<Value>`—prevents runtime type errors
- `Box<RawValue>` for `args`—never parsed by core, zero-copy to dispatcher
- `ToolCallView<'a>` borrows from blocks—zero allocations
- `ToolCallIter` is lazy, not a collected `Vec`
- `impl Display` for text—no intermediate allocations
- **Usage is NOT in AssistantMessage**—it's billing metadata, returned separately

```rust
/// Response from LLM call—separates content from billing metadata.
pub struct LlmResponse {
    pub message: AssistantMessage,
    pub usage: Usage,
}
```

### 3.1.1 Usage Persistence

Usage is billing metadata, not conversational content. It's stored at the **session level**, not per-message:

```rust
/// Session with usage tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub messages: Vec<Message>,
    /// Cumulative token usage across all LLM calls in this session.
    #[serde(default)]
    pub usage: Usage,
    // ... other fields
}

impl Session {
    /// Update cumulative usage after an LLM call.
    pub fn record_usage(&mut self, turn_usage: Usage) {
        self.usage.input_tokens += turn_usage.input_tokens;
        self.usage.output_tokens += turn_usage.output_tokens;
        // cache_read_tokens, cache_write_tokens, etc.
    }

    /// Total token usage for this session. Now reads from session-level field.
    /// BREAKING: Previously derived from per-message usage; now uses Session.usage.
    pub fn total_usage(&self) -> &Usage {
        &self.usage
    }

    /// Total tokens consumed. Convenience for budget checks.
    pub fn total_tokens(&self) -> u64 {
        self.usage.input_tokens + self.usage.output_tokens
    }
}

/// Session metadata for listings/summaries.
/// BREAKING: usage now comes from Session.usage, not aggregated from messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: SessionId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub message_count: usize,
    /// Cumulative usage (copied from Session.usage)
    pub usage: Usage,
}
```

**Agent loop integration**:
```rust
let response = llm_client.call(&request).await?;
self.session.push(Message::Assistant(response.message));
self.session.record_usage(response.usage);  // Cumulative tracking
self.store.save(&self.session).await?;      // Persisted with session
```

**Rationale**: Per-turn usage can be derived from diffs if needed. Cumulative is what matters for budget enforcement and billing. Storing usage per-message would bloat the message log with redundant data.

### 3.2 Tool Result (Simplified)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: String,
    #[serde(default)]
    pub is_error: bool,
    // No meta field - Gemini signatures stay on functionCall, not functionResponse
}
```

**Rationale**: Gemini signatures appear on `functionCall` and `text` parts, but **NEVER on `functionResponse`**. Signature on `functionResponse` is redundant (works but unnecessary). Removing `meta` from `ToolResult` prevents incorrect usage.

### 3.3 Streaming Layer Changes

#### LlmEvent Updates

```rust
// meerkat-client/src/types.rs

use serde_json::value::RawValue;

#[derive(Debug, Clone)]
pub enum LlmEvent {
    /// Incremental text output.
    /// Gemini may include `thoughtSignature` on text parts for continuity.
    TextDelta {
        delta: String,
        /// Provider metadata (Gemini thoughtSignature on text parts)
        meta: Option<ProviderMeta>,
    },

    /// Incremental reasoning/thinking content
    ReasoningDelta { delta: String },

    /// Complete reasoning block (emitted at content_block_stop)
    ReasoningComplete {
        text: String,
        /// Typed provider metadata—each adapter knows its provider
        meta: Option<ProviderMeta>,
    },

    /// Incremental tool call (arrives in pieces)
    ToolCallDelta {
        id: String,
        name: Option<String>,
        args_delta: String,
    },

    /// Complete tool call
    ToolCallComplete {
        id: String,
        name: String,
        /// Raw JSON bytes—never parsed by adapter, only by dispatcher
        args: Box<RawValue>,
        /// Typed provider metadata
        meta: Option<ProviderMeta>,
    },

    /// Token usage update
    UsageUpdate { usage: Usage },

    /// Stream completed
    Done { outcome: LlmDoneOutcome },
}
```

**Design notes**:
- `LlmEvent` is transient (not serialized)—no `Serialize`/`Deserialize`
- `args: Box<RawValue>` avoids parsing; dispatcher parses to tool input struct
- `meta: Option<ProviderMeta>` is typed—each adapter is provider-specific anyway
- No conversion layer needed; typing happens at the source (SSE parsing)
- `TextDelta.meta` is used by Gemini for `thoughtSignature` on text parts; Anthropic passes `None`

#### LlmStreamResult Updates

```rust
/// Aggregated result from streaming an LLM response.
/// Contains only blocks—tool calls are accessed via iterator on AssistantMessage.
#[derive(Debug, Clone)]
pub struct LlmStreamResult {
    /// Ordered blocks (source of truth)
    pub blocks: Vec<AssistantBlock>,
    /// How the turn ended
    pub stop_reason: StopReason,
    /// Token usage (billing metadata, NOT part of domain model)
    pub usage: Usage,
}

impl LlmStreamResult {
    /// Convert to LlmResponse separating content from billing metadata.
    pub fn into_response(self) -> LlmResponse {
        LlmResponse {
            message: AssistantMessage {
                blocks: self.blocks,
                stop_reason: self.stop_reason,
            },
            usage: self.usage,
        }
    }
}

// No separate tool_calls field—derive from blocks via AssistantMessage::tool_calls()
// No content() method—use Display on AssistantMessage or iterate text_blocks()
```

**Rationale**: `tool_calls` was redundant denormalization. The data lives in `blocks`; access it via `AssistantMessage::tool_calls()` iterator. No allocation, no drift risk.

### 3.4 Block Assembly and Ordering

The adapter must track **global arrival order** across all block types, not just tool calls. Blocks must be ordered by when they *started*, not when they completed.

**Key insight**: Text deltas can be appended directly (they arrive in order). But reasoning and tool-use blocks stream incrementally and complete out-of-order. We use a `Slab` with typed keys to preserve start-ordering safely.

```rust
// meerkat-client/src/adapter.rs

use serde_json::value::RawValue;
use slab::Slab;

/// Errors that can occur during stream assembly.
/// Returned to caller, who decides whether to skip, count, or abort.
#[derive(Debug, thiserror::Error)]
pub enum StreamAssemblyError {
    #[error("delta for unknown tool call: {0}")]
    OrphanedToolDelta(String),
    #[error("delta for unknown reasoning block")]
    OrphanedReasoningDelta,
    #[error("duplicate tool call start: {0}")]
    DuplicateToolStart(String),
    #[error("complete event for unknown tool: {0}")]
    UnknownToolComplete(String),
    #[error("finalize args for unknown tool: {0}")]
    UnknownToolFinalize(String),
    #[error("invalid args JSON for tool {id}: {source}")]
    InvalidArgsJson { id: String, source: String },
}

/// Typed key into the block slab—prevents mixing up indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BlockKey(usize);

/// Represents either a finalized block or a placeholder for one still streaming.
enum BlockSlot {
    Finalized(AssistantBlock),
    Pending,
}

/// Buffer for tool call being assembled from streaming deltas.
/// Key (id) is stored in the map, not duplicated here.
struct ToolCallBuffer {
    name: Option<String>,
    args_json: String,
    block_key: BlockKey,  // Typed key into slab—compiler enforces correct usage
}

/// Buffer for reasoning block being assembled.
struct ReasoningBuffer {
    text: String,
    block_key: BlockKey,
}

struct BlockAssembler {
    /// Slab provides stable keys even if items are removed.
    slots: Slab<BlockSlot>,
    /// Map from tool call ID to buffer. ID is the key, not stored in value.
    tool_buffers: IndexMap<String, ToolCallBuffer>,
    reasoning_buffer: Option<ReasoningBuffer>,
}

impl BlockAssembler {
    fn new() -> Self {
        Self {
            slots: Slab::new(),
            tool_buffers: IndexMap::new(),
            reasoning_buffer: None,
        }
    }

    /// Text deltas can always succeed—no Result needed.
    /// `meta` is used by Gemini for thoughtSignature on text parts.
    fn on_text_delta(&mut self, delta: &str, meta: Option<ProviderMeta>) {
        // Find last slot and check if it's a text block we can extend (only if no new meta)
        let should_extend = meta.is_none() && self.slots.iter()
            .last()
            .is_some_and(|(_, slot)| matches!(slot, BlockSlot::Finalized(AssistantBlock::Text { .. })));

        if should_extend {
            // Safe: we just checked this slot exists and is Text
            let last_key = self.slots.iter().last().map(|(k, _)| k);
            if let Some(key) = last_key {
                if let BlockSlot::Finalized(AssistantBlock::Text { text, .. }) = &mut self.slots[key] {
                    text.push_str(delta);
                    return;
                }
            }
        }
        // Insert new text block
        self.slots.insert(BlockSlot::Finalized(
            AssistantBlock::Text { text: delta.into(), meta }
        ));
    }

    fn on_reasoning_start(&mut self) {
        let key = self.slots.insert(BlockSlot::Pending);
        self.reasoning_buffer = Some(ReasoningBuffer {
            text: String::new(),
            block_key: BlockKey(key),
        });
    }

    fn on_reasoning_delta(&mut self, delta: &str) -> Result<(), StreamAssemblyError> {
        let buf = self.reasoning_buffer.as_mut()
            .ok_or(StreamAssemblyError::OrphanedReasoningDelta)?;
        buf.text.push_str(delta);
        Ok(())
    }

    /// Provider adapter converts raw JSON to typed ProviderMeta before calling.
    fn on_reasoning_complete(&mut self, meta: Option<ProviderMeta>) {
        if let Some(buf) = self.reasoning_buffer.take() {
            self.slots[buf.block_key.0] = BlockSlot::Finalized(AssistantBlock::Reasoning {
                text: buf.text,
                meta,
            });
        }
        // Complete without prior start is silently ignored—provider protocol quirk
    }

    fn on_tool_call_start(&mut self, id: String) -> Result<(), StreamAssemblyError> {
        if self.tool_buffers.contains_key(&id) {
            return Err(StreamAssemblyError::DuplicateToolStart(id));
        }
        let key = self.slots.insert(BlockSlot::Pending);
        self.tool_buffers.insert(id, ToolCallBuffer {
            name: None,
            args_json: String::new(),
            block_key: BlockKey(key),
        });
        Ok(())
    }

    fn on_tool_call_delta(
        &mut self,
        id: &str,
        name: Option<&str>,
        args_delta: &str,
    ) -> Result<(), StreamAssemblyError> {
        let buf = self.tool_buffers.get_mut(id)
            .ok_or_else(|| StreamAssemblyError::OrphanedToolDelta(id.to_string()))?;
        if let Some(n) = name {
            buf.name = Some(n.into());
        }
        buf.args_json.push_str(args_delta);
        Ok(())
    }

    /// Convert buffered args_json to RawValue. Called before on_tool_call_complete.
    ///
    /// # Errors
    /// - `UnknownToolFinalize` if no buffer exists for this ID (protocol error)
    /// - `InvalidArgsJson` if the buffered JSON is malformed
    fn finalize_tool_args(&self, id: &str) -> Result<Box<RawValue>, StreamAssemblyError> {
        let buf = self.tool_buffers
            .get(id)
            .ok_or_else(|| StreamAssemblyError::UnknownToolFinalize(id.to_string()))?;

        RawValue::from_string(buf.args_json.clone())
            .map_err(|e| StreamAssemblyError::InvalidArgsJson {
                id: id.to_string(),
                source: e.to_string(),
            })
    }

    /// Provider adapter converts raw JSON to typed ProviderMeta before calling.
    fn on_tool_call_complete(
        &mut self,
        id: String,
        name: String,
        args: Box<RawValue>,
        meta: Option<ProviderMeta>,
    ) -> Result<(), StreamAssemblyError> {
        if let Some((_, buf)) = self.tool_buffers.swap_remove_full(&id) {
            self.slots[buf.block_key.0] = BlockSlot::Finalized(AssistantBlock::ToolUse {
                id,
                name,
                args,
                meta,
            });
            Ok(())
        } else {
            // No prior start—provider that doesn't emit start events
            // Insert at end; ordering may be off but we have the data
            self.slots.insert(BlockSlot::Finalized(AssistantBlock::ToolUse {
                id, name, args, meta,
            }));
            Ok(())
        }
    }

    fn finalize(self) -> Vec<AssistantBlock> {
        // Slab iteration is in insertion order
        self.slots
            .into_iter()
            .filter_map(|(_, slot)| match slot {
                BlockSlot::Finalized(block) => Some(block),
                BlockSlot::Pending => None,
            })
            .collect()
    }
}
```

**Design notes**:
- `Slab<BlockSlot>` provides stable keys even if items were removed
- `BlockKey(usize)` is a newtype—prevents mixing up different indices
- Methods return `Result<(), StreamAssemblyError>` for caller-decided policy
- `ToolCallBuffer` does NOT store `id`—it's the map key, avoiding duplication
- `Box<RawValue>` for args—no parsing in adapter

**Error handling philosophy**: The assembler detects errors and reports them via `Result`. The caller (stream loop) decides whether to skip, count, or abort. This separates mechanism from policy.

**Critical**: Providers that emit `content_block_start` events (Anthropic) naturally call `on_*_start`. For providers that don't emit explicit start events (streaming deltas only), call the start method on first delta for that block ID.

### 3.5 Provider Adapter Changes

#### Anthropic (`anthropic.rs`)

**Required request header** for interleaved thinking (Claude 4 models):

```rust
// When thinking is enabled, add beta header for interleaved thinking support
// Note: This is a static string, so parse failure is a compile-time bug
const INTERLEAVED_THINKING_HEADER: &str = "interleaved-thinking-2025-05-14";

if config.thinking_enabled {
    let header_value = HeaderValue::from_static(INTERLEAVED_THINKING_HEADER);
    headers.insert("anthropic-beta", header_value);
}
```

**Required struct updates** for SSE parsing:

```rust
#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    id: Option<String>,
    name: Option<String>,
    // NEW: for thinking blocks
    thinking: Option<String>,
    signature: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicDelta {
    #[serde(rename = "type")]
    delta_type: String,
    text: Option<String>,
    partial_json: Option<String>,
    stop_reason: Option<String>,
    // NEW: for thinking deltas
    thinking: Option<String>,
    // NEW: signature arrives as separate delta before content_block_stop
    signature: Option<String>,
}
```

**Streaming handler** (block assembly only—events omitted for clarity):

```rust
// State: assembler, current_thinking_signature, current_tool_id, current_tool_name

"content_block_start" => {
    match content_block.block_type.as_str() {
        "thinking" => {
            assembler.on_reasoning_start();
            current_thinking_signature = content_block.signature.take();
        }
        "tool_use" => {
            if let (Some(id), name) = (content_block.id.take(), content_block.name.take()) {
                // Caller decides policy on duplicate start
                if let Err(e) = assembler.on_tool_call_start(id.clone()) {
                    tracing::warn!(?e, "skipping duplicate tool start");
                } else {
                    current_tool_id = Some(id);
                    current_tool_name = name;
                }
            }
        }
        _ => {}
    }
}

"content_block_delta" => {
    match delta.delta_type.as_str() {
        "thinking_delta" => {
            if let Some(text) = delta.thinking.take() {
                // Orphaned reasoning delta—log and continue
                if let Err(e) = assembler.on_reasoning_delta(&text) {
                    tracing::warn!(?e, "orphaned reasoning delta");
                }
            }
        }
        "signature_delta" => {
            // Signature arrives as separate delta just before content_block_stop
            if let Some(sig) = delta.signature.take() {
                current_thinking_signature = Some(sig);
            }
        }
        "text_delta" => {
            if let Some(text) = delta.text.take() {
                assembler.on_text_delta(&text, None);  // Anthropic text has no meta
            }
        }
        "input_json_delta" => {
            if let (Some(id), Some(json)) = (&current_tool_id, delta.partial_json.take()) {
                // Orphaned tool delta—log and continue
                if let Err(e) = assembler.on_tool_call_delta(id, current_tool_name.as_deref(), &json) {
                    tracing::warn!(?e, "orphaned tool delta");
                }
            }
        }
        _ => {}
    }
}

"content_block_stop" => {
    match current_block_type.as_deref() {
        Some("thinking") => {
            // Convert raw signature to typed ProviderMeta
            let meta = current_thinking_signature.take()
                .map(|sig| ProviderMeta::Anthropic { signature: sig });
            assembler.on_reasoning_complete(meta);
        }
        Some("tool_use") => {
            if let (Some(id), Some(name)) = (current_tool_id.take(), current_tool_name.take()) {
                // Finalize args—skip tool call if JSON is irreparably broken
                let args = match assembler.finalize_tool_args(&id) {
                    Ok(args) => args,
                    Err(e) => {
                        tracing::error!(?e, "skipping tool call: args JSON unrecoverable");
                        continue;  // Skip this tool call entirely
                    }
                };
                // Anthropic tool_use blocks don't have signatures—only thinking blocks do
                if let Err(e) = assembler.on_tool_call_complete(id, name, args, None) {
                    tracing::warn!(?e, "tool complete without start");
                }
            }
        }
        _ => {}
    }
}
```

**Note**: This sketch focuses on block assembly. If streaming progress events are needed (for UI), either:
1. Emit events from assembler methods (observer pattern)
2. Clone data for events (acceptable for UI, not hot path)
3. Use `Arc<str>` / `Arc<RawValue>` for shared ownership

**Request building** - render blocks:

```rust
Message::Assistant(a) => {
    let mut content = Vec::new();
    for block in &a.blocks {
        match block {
            AssistantBlock::Text { text, .. } => {
                // Anthropic text blocks don't use meta
                if !text.is_empty() {
                    content.push(json!({"type": "text", "text": text}));
                }
            }
            AssistantBlock::Reasoning { text, meta } => {
                // Claude 4.5 REQUIRES signature on thinking blocks for replay
                let Some(ProviderMeta::Anthropic { signature }) = meta else {
                    tracing::error!("thinking block missing Anthropic signature, skipping");
                    continue;
                };
                content.push(json!({
                    "type": "thinking",
                    "thinking": text,
                    "signature": signature
                }));
            }
            AssistantBlock::ToolUse { id, name, args, .. } => {
                content.push(json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": args
                }));
            }
        }
    }
    messages.push(json!({"role": "assistant", "content": content}));
}

// ToolResults rendering - MUST immediately follow assistant with tool_use
// tool_result blocks MUST be first in user content array
Message::ToolResults { results } => {
    let content: Vec<Value> = results.iter().map(|r| {
        json!({
            "type": "tool_result",
            "tool_use_id": r.tool_use_id,
            "content": r.content,
            "is_error": r.is_error
        })
    }).collect();
    messages.push(json!({"role": "user", "content": content}));
}
```

**Ordering requirement**: Anthropic requires tool_result blocks to appear in a user message immediately following the assistant message that contained the tool_use blocks. The tool_result must reference the exact `tool_use_id`.

#### Gemini (`gemini.rs`)

**Request building** - signature on functionCall only:

```rust
Message::Assistant(a) => {
    let mut parts = Vec::new();
    for block in &a.blocks {
        match block {
            AssistantBlock::Text { text, meta } => {
                if !text.is_empty() {
                    let mut part = json!({"text": text});
                    // Gemini may have thoughtSignature on text parts for continuity
                    if let Some(ProviderMeta::Gemini { thought_signature }) = meta {
                        part["thoughtSignature"] = json!(thought_signature);
                    }
                    parts.push(part);
                }
            }
            AssistantBlock::Reasoning { text, .. } => {
                // Gemini doesn't accept reasoning blocks back
                // Just include as text if needed for context
                if !text.is_empty() {
                    parts.push(json!({"text": format!("[Reasoning: {}]", text)}));
                }
            }
            AssistantBlock::ToolUse { id, name, args, meta } => {
                let mut part = json!({"functionCall": {"name": name, "args": args}});
                // Only add signature if present (first parallel call has it)
                if let Some(ProviderMeta::Gemini { thought_signature }) = meta {
                    part["thoughtSignature"] = json!(thought_signature);
                }
                parts.push(part);
            }
        }
    }
    contents.push(json!({"role": "model", "parts": parts}));
}

// ToolResults - NO signature
Message::ToolResults { results } => {
    let parts: Vec<Value> = results.iter().map(|r| {
        // Gemini needs function name, fall back to tool_use_id if not in map
        let function_name = match tool_name_by_id.get(&r.tool_use_id) {
            Some(name) => name.clone(),
            None => r.tool_use_id.clone(),
        };
        json!({
            "functionResponse": {
                "name": function_name,
                "response": {"content": r.content, "error": r.is_error}
            }
            // NO thoughtSignature here - verified not needed
        })
    }).collect();
    contents.push(json!({"role": "user", "parts": parts}));
}
```

#### OpenAI (`openai.rs`) - Responses API Migration

**This is a significant change.** The Responses API uses a different schema:

```rust
// Request format for Responses API
let body = json!({
    "model": request.model,
    "input": convert_to_responses_input(&request.messages),
    "tools": convert_tools(&request.tools),
    "reasoning": { "effort": "medium", "summary": "auto" },  // Enable reasoning
    // CRITICAL: Request encrypted_content for stateless replay
    "include": ["reasoning.encrypted_content"],
    // ... other params
});

fn convert_to_responses_input(messages: &[Message]) -> Vec<Value> {
    let mut items = Vec::new();

    for msg in messages {
        match msg {
            Message::System(s) => {
                items.push(json!({
                    "type": "message",
                    "role": "system",
                    "content": s.content
                }));
            }
            Message::User(u) => {
                items.push(json!({
                    "type": "message",
                    "role": "user",
                    "content": u.content
                }));
            }
            Message::Assistant(a) => {
                for block in &a.blocks {
                    match block {
                        AssistantBlock::Text { text, .. } => {
                            // OpenAI text has no provider-specific meta
                            items.push(json!({
                                "type": "message",
                                "role": "assistant",
                                "content": text
                            }));
                        }
                        AssistantBlock::Reasoning { text, meta } => {
                            // OpenAI requires id and summary for reasoning items
                            if let Some(ProviderMeta::OpenAi { id, encrypted_content }) = meta {
                                let mut item = json!({
                                    "type": "reasoning",
                                    "id": id,
                                    "summary": [{"type": "summary_text", "text": text}]
                                });
                                if let Some(enc) = encrypted_content {
                                    item["encrypted_content"] = json!(enc);
                                }
                                items.push(item);
                            }
                            // Skip reasoning blocks without OpenAI metadata (wrong provider)
                        }
                        AssistantBlock::ToolUse { id, name, args, .. } => {
                            // args is Box<RawValue>—already JSON bytes
                            // .get() returns &str of the raw JSON, no re-serialization
                            items.push(json!({
                                "type": "function_call",
                                "call_id": id,
                                "name": name,
                                "arguments": args.get()  // &str, no allocation
                            }));
                        }
                    }
                }
            }
            Message::ToolResults { results } => {
                for r in results {
                    items.push(json!({
                        "type": "function_call_output",
                        "call_id": r.tool_use_id,
                        "output": r.content
                    }));
                }
            }
        }
    }
    items
}
```

**Response parsing** - Responses API returns items:

```rust
// Parse Responses API output items
// Note: `content` is an ARRAY of parts, not a string!
for item in output_items {
    match item["type"].as_str() {
        Some("message") => {
            // content is an array: [{"type": "output_text", "text": "..."}, ...]
            if let Some(content_parts) = item["content"].as_array() {
                for part in content_parts {
                    match part["type"].as_str() {
                        Some("output_text") => {
                            if let Some(text) = part["text"].as_str() {
                                yield LlmEvent::TextDelta { delta: text.to_string(), meta: None };
                            }
                        }
                        Some("refusal") => {
                            // Model refused to respond - treat as text for now
                            if let Some(refusal) = part["refusal"].as_str() {
                                yield LlmEvent::TextDelta { delta: refusal.to_string(), meta: None };
                            }
                        }
                        _ => {} // Other content types (future expansion)
                    }
                }
            }
        }
        Some("reasoning") => {
            // Required: reasoning item ID for replay
            let Some(reasoning_id) = item["id"].as_str() else {
                tracing::warn!("reasoning item missing id, skipping");
                continue;
            };

            // Extract summary text (may be empty if only encrypted_content)
            let mut summary_text = String::new();
            if let Some(summaries) = item["summary"].as_array() {
                for summary in summaries {
                    if let Some(text) = summary["text"].as_str() {
                        if !summary_text.is_empty() {
                            summary_text.push('\n');
                        }
                        summary_text.push_str(text);
                    }
                }
            }

            // encrypted_content is optional (only present if requested via include param)
            let encrypted = item.get("encrypted_content")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            yield LlmEvent::ReasoningComplete {
                text: summary_text,
                meta: Some(ProviderMeta::OpenAi {
                    id: reasoning_id.to_string(),
                    encrypted_content: encrypted,
                }),
            };
        }
        Some("function_call") => {
            // Extract required fields, skip malformed items
            let Some(call_id) = item["call_id"].as_str() else {
                tracing::warn!("function_call missing call_id");
                continue;
            };
            let Some(name) = item["name"].as_str() else {
                tracing::warn!(call_id, "function_call missing name");
                continue;
            };
            // arguments is a JSON string—convert to RawValue without parsing
            let args: Box<RawValue> = match item["arguments"].as_str() {
                Some(args_str) => match RawValue::from_string(args_str.to_string()) {
                    Ok(raw) => raw,
                    Err(e) => {
                        tracing::error!(call_id, "invalid args JSON, skipping: {e}");
                        continue;
                    }
                },
                None => {
                    tracing::error!(call_id, "missing arguments field, skipping");
                    continue;
                }
            };
            yield LlmEvent::ToolCallComplete {
                id: call_id.to_string(),
                name: name.to_string(),
                args,
                meta: None,
            };
        }
        _ => {}
    }
}
```

---

## 4. Session Persistence

### 4.1 Schema Change

No versioned migration—clean break:

```rust
pub enum Message {
    System(SystemMessage),
    User(UserMessage),
    Assistant(AssistantMessage),  // Now has blocks: Vec<AssistantBlock>
    ToolResults { results: Vec<ToolResult> },
}
```

### 4.2 Checkpointing Strategy

A **turn** = one complete LLM call → response (all blocks finalized).

Checkpoint points (via `store.save()`):

1. **After assistant message pushed** (before tool execution)
2. **After tool results pushed** (before next LLM call)
3. **On run completion**

```rust
// In agent loop:
self.session.push(Message::Assistant(assistant_msg));
self.store.save(&self.session).await?;  // Checkpoint 1

// After tools:
self.session.push(Message::ToolResults { results });
self.store.save(&self.session).await?;  // Checkpoint 2
```

**Note**: Messages between checkpoints can be lost on crash. This is an acceptable edge case for now.

---

## 5. Compaction Strategy (Sketch)

With blocks preserved, core-level compaction can:

1. **Read recent turns' blocks** including reasoning notes
2. **Generate compacted summary** via extraction turn
3. **Preserve critical metadata** (tool signatures) for replay

**Block priority for retention**:
1. User messages (highest)
2. Tool results
3. Assistant text
4. Reasoning/thinking (can be summarized)

**Not in scope for this spec**: Full compaction implementation.

---

## 6. Test Plan

### 6.1 Unit Tests

1. **Ordering tests**: Blocks appear in stable order from interleaved streaming events
2. **Block round-trip**: `AssistantBlock` → serialize → deserialize → identical
3. **Meta preservation**: Opaque meta survives serialization
4. **Tool extraction**: `AssistantMessage::tool_calls()` returns correct data

### 6.2 Integration Tests (per provider)

**Anthropic**:
- Blocks → request JSON contains thinking/tool_use in correct order
- Thinking signatures preserved and returned
- `AnthropicEvent` structs parse thinking block SSE events

**Gemini**:
- `ToolUse.meta` and `Text.meta` → `thoughtSignature` on functionCall and text parts
- No signature on functionResponse part (NEVER)
- Parallel calls: only first has signature (verified behavior)

**OpenAI**:
- Responses API: reasoning items captured and replayed
- Function calls/outputs use correct item format

### 6.3 End-to-End Tests

1. `rkat run` with thinking enabled → `rkat resume` → thinking blocks replayed
2. Multi-step tool loop → all blocks and signatures preserved
3. Crash recovery: interrupt mid-tool → resume from checkpoint

---

## 7. Implementation Checklist

### Phase 1: Core Types
- [ ] Add `indexmap` and `slab` dependencies to `meerkat-client`
- [ ] Add `AssistantBlock` enum to `meerkat-core/src/types.rs`
- [ ] Update `AssistantMessage` to use blocks
- [ ] Add `ToolCallView` struct with `parse_args<T>()` method
- [ ] Update `AgentToolDispatcher::dispatch()` to take `ToolCallView<'_>`
- [ ] Keep `ToolCall` struct, add extraction method to `AssistantMessage`
- [ ] Remove `thought_signature` field from `ToolCall` and `ToolResult`
- [ ] Add `Session.usage` field, `record_usage()`, `total_usage()`, `total_tokens()`
- [ ] Add `SessionMeta` struct with usage field
- [ ] Update `LlmEvent` with `ReasoningDelta`/`ReasoningComplete`
- [ ] Update `LlmStreamResult` to include blocks

### Phase 2: Streaming & Aggregation
- [ ] Implement `BlockAssembler` in adapter
- [ ] Replace `HashMap` with `IndexMap` in adapter and provider clients
- [ ] Update Anthropic SSE event structs for thinking
- [ ] Update Anthropic streaming to capture thinking blocks

### Phase 3: Provider Request Building
- [ ] Update Anthropic request builder to render blocks
- [ ] Update Gemini request builder (sig on functionCall only)
- [ ] Migrate OpenAI client to Responses API

### Phase 4: Integration
- [ ] Update agent loop to use new types
- [ ] Add checkpoint saves to agent loop
- [ ] Update session store for new schema
- [ ] Add model validation in `AgentBuilder::build()` with allowlist
- [ ] Return `Err(UnsupportedModel)` for rejected models

### Phase 5: Testing
- [ ] Add unit tests for block ordering
- [ ] Add integration tests per provider
- [ ] Add e2e resume tests

---

## Appendix A: Core Loop Architecture

### A.1 State Machine

The agent loop (`meerkat-core/src/agent/state.rs`) uses these states:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                            STATE MACHINE                                    │
│                                                                             │
│   ┌──────────────┐                                                          │
│   │  CallingLlm  │◄────────────────────────────────────────────┐            │
│   └──────┬───────┘                                             │            │
│          │                                                     │            │
│          ▼                                                     │            │
│   ┌──────────────┐    ┌────────────────┐    ┌──────────────┐  │            │
│   │WaitingForOps │───►│ DrainingEvents │───►│ CallingLlm   │──┘            │
│   └──────────────┘    └───────┬────────┘    └──────────────┘               │
│                               │                                             │
│                               ▼                                             │
│                        ┌──────────────┐                                     │
│                        │  Completed   │                                     │
│                        └──────────────┘                                     │
│                                                                             │
│   Also: Cancelling, ErrorRecovery (not shown for simplicity)                │
└─────────────────────────────────────────────────────────────────────────────┘
```

### A.2 Turn Cycle

```
1. CallingLlm
   └─► Call LLM with retry logic (state.rs:19-62)
   └─► Receive LlmStreamResult (now with blocks)

2. If tool_calls present:
   └─► Push Message::Assistant with blocks (state.rs:131-137)
   └─► Checkpoint save
   └─► Transition to WaitingForOps (state.rs:149)
   └─► Execute ALL tools in PARALLEL via join_all (state.rs:163-188)
   └─► Push Message::ToolResults (state.rs:259-262)
   └─► Checkpoint save
   └─► Transition to DrainingEvents (state.rs:265)

3. DrainingEvents (TURN BOUNDARY)
   └─► drain_comms_inbox() - STEERING INJECTION POINT (state.rs:270)
   └─► collect_sub_agent_results() (state.rs:273)
   └─► Transition back to CallingLlm (state.rs:290)
   └─► Increment turn counter

4. If no tool_calls:
   └─► Push final Message::Assistant (state.rs:295-300)
   └─► Transition to Completed
```

---

## Appendix B: Verified Provider Behaviors

### B.1 Gemini thought_signature (Empirically Verified)

Test script: `scripts/test_gemini_thought_signature.py`

| Scenario | HTTP Status | Result |
|----------|-------------|--------|
| Signature on functionCall only | 200 | **PASS** - correct approach |
| Signature on functionResponse only | 400 | **FAIL** - must be on functionCall |
| Signature on BOTH | 200 | PASS (redundant) |
| NO signature anywhere | 400 | **FAIL** - required for Gemini 3 |

### B.2 Gemini Parallel Calls (Empirically Verified)

Test script: `scripts/test_gemini_parallel_calls.py`

```
3 parallel function calls:
  Call 1 (get_weather):   HAS signature
  Call 2 (get_time):      NO signature
  Call 3 (get_population): NO signature

Conclusion: Only FIRST parallel call has thoughtSignature
```

### B.3 Provider Rendering Differences

| Aspect | Anthropic | OpenAI (Responses) | Gemini |
|--------|-----------|-------------------|--------|
| System message | Top-level `system` | Item with `role: system` | Top-level `systemInstruction` |
| Assistant role | `"assistant"` | `"assistant"` | `"model"` |
| Block format | `content[]` array | Separate items | `parts[]` array |
| Tool call format | `tool_use` block | `function_call` item | `functionCall` part |
| Args format | `input` (JSON object) | `arguments` (JSON string!) | `args` (JSON object) |
| Tool result | `tool_result` block | `function_call_output` item | `functionResponse` part |
| Continuity token | Thinking signature | Encrypted reasoning | `thoughtSignature` |

---

## Appendix C: Decision Record

### C.1 Blocks + Separate ToolCall

**Decision**: Keep both `AssistantBlock::ToolUse` in blocks AND `ToolCall` as a separate struct.

**Rationale**: The core loop needs a stable dispatch interface (`id`, `name`, `args`). Blocks are the source of truth for storage/replay. `AssistantMessage::tool_calls()` bridges the two.

### C.2 ToolResult.meta Removal

**Decision**: Remove `meta` from `ToolResult`.

**Rationale**: Empirically verified that Gemini signatures belong on `functionCall` only. Signature on `functionResponse` works but is redundant. Removing `meta` from `ToolResult` prevents incorrect usage patterns.

### C.3 Responses API Only

**Decision**: OpenAI implementation uses Responses API exclusively.

**Rationale**: Chat Completions API drops reasoning between calls and lacks reasoning summary support. Responses API provides proper reasoning persistence. No point maintaining legacy support.

### C.4 Finalized Blocks per Turn

**Decision**: Store finalized blocks after each LLM response, with checkpoints.

**Rationale**:
- Simpler than event-log approach
- Checkpoints cover main crash scenarios
- Message loss between checkpoints is acceptable edge case
- "Turn" = one complete LLM call → response with all blocks finalized
