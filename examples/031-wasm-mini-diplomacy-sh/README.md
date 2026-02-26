# 031 — WASM Mini Diplomacy Arena (Shell + Web)

Flagship browser example: two WASM mobs play a territory-control diplomacy game with live network graph, chatter feed, and replay export/import.

## Why this is cool
- Zero-install browser runtime from `rkat mob web build`
- Per-mob model selection (`claude-opus-4-6`, `gpt-5.2`, `gemini-3.1-pro-preview`)
- Meerkat skills split by role: shared rules + planner + operator
- Real-time observability: board, comms graph, chatter timeline, replay log

## Concepts
- Portable `.mobpack` artifacts for both teams
- Browser-safe WASM runtime lifecycle APIs (`init/start/submit/poll/snapshot/restore`)
- Deterministic simultaneous-order resolver for reproducible replays
- UI-first orchestration of two independent mob runtimes

## Prerequisites
```bash
cargo install rkat wasm-pack
node --version   # 20+
npm --version
```

## Run
```bash
chmod +x examples.sh && ./examples.sh
```

Then open the printed local URL and click **Start Match**.

## Auth modes in UI
- `browser_byok`: keys in browser session only (demo mode)
- `proxy`: route through backend endpoint (enterprise mode)

This demo focuses on runtime orchestration and observability; key inputs are optional unless you wire live provider execution in your deployment.
