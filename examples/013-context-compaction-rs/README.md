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
Current context/last-request token pressure exceeds auto_compact_threshold →
  Compactor selects messages → LLM summarizes →
  Old messages replaced with summary → Agent continues
```

## Preservation Rules
- Unkeyed System messages and the latest version of each keyed prompt are
  preserved in order; superseded keyed versions can be compacted
- `recent_turn_budget` is an upper bound on recent complete turns retained
  verbatim (including their tool-call/result structure and attached injected
  context), not a minimum guarantee. Compaction removes at least one live turn
  to make progress and can retain fewer turns to fit the retained-history byte
  budget under request-capacity pressure.
- Tool call/result pairs are kept together
- Compaction summaries are themselves compactable

The token threshold measures current context/last-request pressure, not
lifetime cumulative billed input tokens. Whether this short live conversation
triggers compaction depends on the provider's responses and token accounting.

## Run
```bash
# From the repository root
ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat \
  --example 013-context-compaction --features jsonl-store,session-compaction
```
