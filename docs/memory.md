# Memory & Compaction

Meerkat provides two complementary systems for managing long-running conversations:

1. **Context compaction** -- automatically summarizes conversation history when the context window fills up, preserving recent turns and discarding older ones.
2. **Semantic memory** -- indexes discarded messages so the agent can retrieve past context on demand via the `memory_search` tool.

Together they allow agents to maintain coherent multi-turn sessions that exceed any single model's context limit.

## Feature Flags

Both systems require compile-time features and per-request enablement.

| Feature flag | Cargo feature | What it enables |
|---|---|---|
| Compaction strategy | `session-compaction` | `DefaultCompactor` in `meerkat-session` |
| Memory store backend | `memory-store` | `meerkat-store/memory` backend |
| Memory + compaction wiring | `memory-store-session` | `HnswMemoryStore`, `MemorySearchDispatcher`, agent loop integration |

The CLI's default features include `sub-agents` and `skills` but **not** `session-compaction` or `memory-store`. To enable them:

```bash
cargo build -p meerkat-cli --features session-compaction,memory-store
```

At runtime, the `AgentFactory` has an `enable_memory` flag (default: `false`). Per-request builds can override it via `AgentBuildConfig::override_memory`.

```rust
let factory = AgentFactory::new(store_path)
    .builtins(true)
    .memory(true);  // enable semantic memory + compaction

// Per-request override:
let mut build_config = AgentBuildConfig::new("claude-sonnet-4-5");
build_config.override_memory = Some(true);
```

## CompactionConfig

Defined in `meerkat-core/src/compact.rs`. Controls when and how compaction runs.

| Field | Type | Default | Description |
|---|---|---|---|
| `auto_compact_threshold` | `u64` | `100_000` | Compaction triggers when `last_input_tokens >= threshold` OR `estimated_history_tokens >= threshold`. |
| `recent_turn_budget` | `usize` | `4` | Number of recent complete turns to retain after compaction. A turn is a User message followed by all subsequent non-User messages until the next User message. |
| `max_summary_tokens` | `u32` | `4096` | Maximum tokens the LLM may produce for the compaction summary. |
| `min_turns_between_compactions` | `u32` | `3` | Minimum turns that must elapse between consecutive compactions (loop guard to prevent runaway compaction). |

## How Compaction Triggers

Compaction is checked at every turn boundary, just before the `CallingLlm` state in the agent loop (`meerkat-core/src/agent/state.rs`).

The decision flow:

1. **Never on turn 0** -- the first turn always skips compaction.
2. **Loop guard** -- if compaction occurred at turn N, no compaction until turn N + `min_turns_between_compactions`.
3. **Dual threshold** -- compaction triggers if EITHER:
   - `last_input_tokens >= auto_compact_threshold` (input tokens from the last LLM response), OR
   - `estimated_history_tokens >= auto_compact_threshold` (JSON bytes of all messages / 4).

## What Happens During Compaction

When compaction triggers (`meerkat-core/src/agent/compact.rs`):

1. **CompactionStarted event** is emitted with input/estimated token counts and message count.
2. The current conversation history plus a **compaction prompt** is sent to the LLM with no tools and `max_summary_tokens` as the response limit.
3. **On failure**: a CompactionFailed event is emitted and the session is not mutated (safe failure).
4. **On success**: `DefaultCompactor::rebuild_history` produces new messages:
   - System prompt is preserved verbatim (if present).
   - A summary message is injected as a User message with the prefix `[Context compacted]`.
   - The last `recent_turn_budget` complete turns are retained.
   - All other messages become `discarded`.
5. The session messages are replaced with the rebuilt history.
6. Compaction usage is recorded against the session and budget.
7. **CompactionCompleted event** is emitted with summary token count and before/after message counts.

### The Compaction Prompt

The `DefaultCompactor` sends this prompt to the LLM (defined in `meerkat-session/src/compactor.rs`):

> You are performing a CONTEXT COMPACTION. Your job is to create a handoff summary so work can continue seamlessly.
>
> Include:
> - Current progress and key decisions made
> - Important context, constraints, or user preferences discovered
> - What remains to be done (clear next steps)
> - Any critical data, file paths, examples, or references needed to continue
> - Tool call patterns that worked or failed
>
> Be concise and structured. Prioritize information the next context needs to act, not narrate.

## Memory Indexing After Compaction

When both a `Compactor` and a `MemoryStore` are wired into the agent, discarded messages are indexed into semantic memory after compaction completes. This happens asynchronously (fire-and-forget via `tokio::spawn`).

For each discarded message:
- The message's indexable text content is extracted via `message.as_indexable_text()`.
- If non-empty, it is indexed with `MemoryMetadata` containing the session ID, current turn number, and a timestamp.

This means previously discarded conversation content becomes searchable via the `memory_search` tool.

## The `memory_search` Tool

Defined in `meerkat-memory/src/tool.rs` as `MemorySearchDispatcher`. It implements `AgentToolDispatcher` and provides a single tool.

### Tool Definition

| Property | Value |
|---|---|
| Name | `memory_search` |
| Description | Search semantic memory for past conversation content. Memory contains text from earlier conversation turns that were compacted away to save context space. |

### Parameters

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `query` | `string` | Yes | -- | Natural language search query describing what you want to recall. |
| `limit` | `integer` | No | `5` | Maximum number of results to return. Capped at 20. |

### Response Format

Returns a JSON array of result objects:

```json
[
  {
    "content": "The text content of the memory entry",
    "score": 0.85,
    "session_id": "019467d9-7e3a-7000-8000-000000000000",
    "turn": 3
  }
]
```

- `score`: 0.0 (no match) to 1.0 (exact match). Typical useful matches are above 0.7.
- `session_id`: The session the memory originated from (enables cross-session recall).
- `turn`: The turn number within the session when the memory was indexed.

## Memory Store Implementations

### HnswMemoryStore (Production)

Defined in `meerkat-memory/src/hnsw.rs`. Uses:
- **hnsw_rs** (v0.3) for approximate nearest-neighbor search with cosine distance.
- **redb** for persistent metadata and text storage.

Storage layout: `.rkat/memory/memory.redb`

Key characteristics:
- **Embedding**: Bag-of-words TF with hash-based dimensionality reduction (4096-dimensional vectors, L2-normalized). Each word is hashed to a bucket and its presence increments that dimension.
- **Persistence**: Data survives process restart. On `open()`, all existing entries are re-indexed into the HNSW graph from redb.
- **Score conversion**: HNSW cosine distance (0 = identical, 2 = opposite) is converted to a 0..1 similarity score: `score = 1.0 - (distance / 2.0)`.
- **Thread safety**: Insertions are serialized via a `Mutex` to couple point ID allocation with successful writes. Searches use a `RwLock` for concurrent reads.
- **Constants**: `MAX_NB_CONNECTION = 16`, `MAX_LAYER = 16`, `EF_CONSTRUCTION = 200`, `DEFAULT_MAX_ELEMENTS = 100_000`.

> **Note**: The current embedding is a baseline bag-of-words approach. The code comments indicate this should be replaced with a proper embedding model (e.g., sentence-transformers via ONNX runtime) for production quality.

### SimpleMemoryStore (Test-only)

Defined in `meerkat-memory/src/simple.rs`. In-memory substring matching -- not suitable for production.

- Stores entries in a `Vec<MemoryEntry>` behind a `RwLock`.
- Search: lowercases both query and content, counts matching words, scores as `matching_words / total_query_words`.
- No persistence -- data is lost when the process exits.

## Factory Wiring

In `meerkat/src/factory.rs`, the `AgentFactory::build_agent()` method wires memory and compaction at steps 12b and 12c:

### Step 12b: Memory Store + memory_search Tool

Gated behind `#[cfg(feature = "memory-store-session")]` AND `effective_memory == true`:

1. Opens (or creates) an `HnswMemoryStore` at `{store_path}/memory/`.
2. Sets it on the `AgentBuilder` via `.memory_store(store)`.
3. Creates a `MemorySearchDispatcher` wrapping the same store.
4. Composes it into the tool dispatcher via `ToolGatewayBuilder`, adding `memory_search` alongside all other tools.
5. Tool guidance reaches the model via the embedded `memory-retrieval` skill (loaded in step 11), not through usage_instructions strings.

### Step 12c: Compactor

Gated behind `#[cfg(feature = "session-compaction")]`:

1. Creates a `DefaultCompactor` with `CompactionConfig::default()`.
2. Sets it on the `AgentBuilder` via `.compactor(compactor)`.

### Skill Integration

The `meerkat-memory` crate registers an embedded skill via `inventory::submit!`:

- **Skill ID**: `memory-retrieval`
- **Requires capability**: `memory_store`
- **Content**: The `SKILL.md` file explains how semantic memory works with compaction.

The skill is automatically included in the system prompt when the `memory_store` capability is active.

## Examples

### Enabling Memory in the CLI

```bash
# Build with memory + compaction features
cargo build -p meerkat-cli --features session-compaction,memory-store

# Run with memory enabled (if the CLI surfaces this flag)
rkat run --memory "Analyze this large codebase..."
```

### Programmatic Usage

```rust
use meerkat::{AgentFactory, AgentBuildConfig};

let factory = AgentFactory::new(".rkat/sessions")
    .project_root(".")
    .builtins(true)
    .memory(true);

let mut build_config = AgentBuildConfig::new("claude-sonnet-4-5");
// Memory + compaction are wired automatically when:
//   1. The factory has memory(true)
//   2. The binary is compiled with session-compaction + memory-store-session features

let agent = factory.build_agent(build_config, &config).await?;
```

### Custom CompactionConfig

```rust
use meerkat_core::CompactionConfig;
use meerkat_session::DefaultCompactor;

let compactor = DefaultCompactor::new(CompactionConfig {
    auto_compact_threshold: 50_000,   // Compact earlier
    recent_turn_budget: 6,            // Keep more recent turns
    max_summary_tokens: 8192,         // Allow longer summaries
    min_turns_between_compactions: 5, // More spacing between compactions
});
```

## Architecture Summary

```
meerkat-core/src/compact.rs       -- Compactor trait, CompactionConfig, CompactionContext
meerkat-core/src/memory.rs        -- MemoryStore trait, MemoryMetadata, MemoryResult
meerkat-core/src/agent/compact.rs -- run_compaction(), estimate_tokens(), CompactionOutcome
meerkat-core/src/agent/state.rs   -- Agent loop integration (check + trigger compaction)
meerkat-session/src/compactor.rs  -- DefaultCompactor implementation
meerkat-memory/src/hnsw.rs        -- HnswMemoryStore (production)
meerkat-memory/src/simple.rs      -- SimpleMemoryStore (test-only)
meerkat-memory/src/tool.rs        -- MemorySearchDispatcher (memory_search tool)
meerkat/src/factory.rs            -- Factory wiring (steps 12b, 12c)
```
