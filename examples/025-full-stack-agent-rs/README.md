# 025 — Full-Stack Agent (Rust)

The reference architecture for production Meerkat agents. Combines every
feature: custom tools, built-in tools, skills, budget control, session
persistence, event streaming, and composable dispatchers.

## Features Used
- `AgentBuilder` with full configuration
- `CompositeDispatcher` — merge built-in + domain tools
- `BudgetLimits` — production cost control
- Skills — injected behavioral instructions
- Event streaming — real-time monitoring
- Session persistence — survives restarts

## Architecture
```
┌─────────────────────────────────┐
│         Full-Stack Agent        │
│                                 │
│  ┌─────────┐   ┌────────────┐  │
│  │ Builtins│   │ Domain     │  │
│  │ tasks   │   │ search_docs│  │
│  │ wait    │   │ create_tkt │  │
│  └────┬────┘   └─────┬──────┘  │
│       └───────┬───────┘         │
│        Composite Dispatcher     │
│               │                 │
│  ┌────────────┴──────────────┐  │
│  │      Agent Loop           │  │
│  │  LLM → Tools → Events    │  │
│  │  Budget │ Retry │ Hooks   │  │
│  └───────────────────────────┘  │
│               │                 │
│  ┌────────────┴──────────────┐  │
│  │    JsonlStore (persist)   │  │
│  └───────────────────────────┘  │
└─────────────────────────────────┘
```

## Run
```bash
# This is a reference implementation. For runnable examples, see meerkat/examples/.
```
