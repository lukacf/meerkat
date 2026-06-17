# 013 — Context Compaction (Rust)

Run agents indefinitely without exceeding the context window. The compactor
automatically summarizes old messages while preserving critical context.

## Concepts
- `DefaultCompactor` — the built-in compaction strategy
- `CompactionConfig` — threshold, summary size, preservation rules
- Compaction events in the event stream
- Infinite conversation support

## How It Works
```
Messages accumulate → auto_compact_threshold exceeded →
  Compactor selects messages → LLM summarizes →
  Old messages replaced with summary → Agent continues
```

## Preservation Rules
- System prompt is always preserved
- The N most recent message pairs are preserved
- Tool call/result pairs are kept together
- Compaction summaries are themselves compactable

## Run
```bash
# From the repository root
ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat \
  --example 013-context-compaction --features jsonl-store,session-compaction
```
