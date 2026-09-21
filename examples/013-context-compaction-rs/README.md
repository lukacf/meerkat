# 013 — Context Compaction (Rust)

Keep longer conversations within the context window by summarizing older
messages while preserving recent and structurally important context.

## Concepts
- `DefaultCompactor` — the built-in compaction strategy
- `CompactionConfig` — threshold, summary size, preservation rules
- Compaction events in the event stream
- Repeated compaction for long-running conversations

## How It Works
```
Messages accumulate → auto_compact_threshold exceeded →
  Compactor selects messages → LLM summarizes →
  Old messages replaced with summary → Agent continues
```

## Preservation Rules
- System prompt is always preserved
- Up to `recent_turn_budget` recent complete conversational turns are retained,
  including their tool-call/result structure and attached injected context.
  Fewer turns may be retained to summarize actual old content and fit the
  retained-history byte budget.
- Tool call/result pairs are kept together
- Compaction summaries are themselves compactable

## Run
```bash
# From the repository root
ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat \
  --example 013-context-compaction --features jsonl-store,session-compaction
```
