# 025 - Composed Agent (Rust)

A focused standalone example that composes built-in tools, two domain tools,
budget limits, a file-backed store, inline behavior instructions, and event
streaming. It is not an exhaustive production reference.

## Offline Domain-Tool Fixtures

`search_docs` returns three canned documentation entries; it never queries an
internal documentation service. Its optional `limit` defaults to 5 and caps
returned entries (0 returns none). `total` always reports the three available
fixture entries, before limiting.

`create_ticket` returns a **simulated** ticket with a fixed ID and timestamp.
It does not create or update anything in an issue tracker. Both tools disclose
their simulation in their model-facing descriptions and result data; terminal
output also labels the demonstration. LLM calls still require provider access.

## Features Used
- `AgentBuilder` configuration
- `CompositeDispatcher` - merge built-in and domain tools
- `BudgetLimits` - cap total tokens and tool calls
- Inline system-prompt behavior instructions
- Event streaming with `spawn_event_logger`
- `JsonlStore` in a temporary directory for this run

## Architecture
```
┌─────────────────────────────────┐
│         Composed Agent          │
│                                 │
│  ┌─────────┐   ┌────────────┐  │
│  │ Builtins│   │ Domain     │  │
│  │ tasks   │   │ search_docs│  │
│  │ datetime│   │ create_tkt │  │
│  └────┬────┘   └─────┬──────┘  │
│       └───────┬───────┘         │
│        Composite Dispatcher     │
│               │                 │
│  ┌────────────┴──────────────┐  │
│  │      Agent Loop           │  │
│  │  LLM → Tools → Events    │  │
│  │  Budget + event stream    │  │
│  └───────────────────────────┘  │
│               │                 │
│  ┌────────────┴──────────────┐  │
│  │ JsonlStore (temporary dir)│  │
│  └───────────────────────────┘  │
└─────────────────────────────────┘
```

This program does not configure the canonical skill engine, hooks, structured
output, shell, MCP, comms, delegation, or runtime-backed restart recovery.
Follow the focused examples for those surfaces.

## Run
```bash
# From the repository root
ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat \
  --example 025-full-stack-agent --features jsonl-store
```
