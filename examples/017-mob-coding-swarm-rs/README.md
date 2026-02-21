# 017 — Mob: Coding Swarm (Rust)

A coordinated team of agents where a lead orchestrator manages worker agents
for coding tasks. The mob runtime handles spawning, wiring, and lifecycle.

## Concepts
- `MobDefinition` — declarative mob structure (profiles, wiring, skills)
- `Prefab` — built-in mob templates (CodingSwarm, CodeReview, etc.)
- `MobBuilder` — create or resume a mob
- `MobHandle` — interact with a running mob
- Mob tools — spawn, retire, wire, status
- Task board — shared task tracking across agents

## Mob Architecture
```
                    ┌──────────────┐
    User prompt ──→ │     Lead     │ (claude-opus-4-6)
                    │ Orchestrator │
                    │              │
                    │ mob.spawn()  │
                    │ mob.wire()   │
                    └──────┬───────┘
                     auto-wire
              ┌────────┼────────┐
              ↓        ↓        ↓
         ┌────────┐ ┌────────┐ ┌────────┐
         │Worker 1│ │Worker 2│ │Worker 3│ (claude-sonnet-4-5)
         │  shell │ │  shell │ │  shell │
         │  comms │ │  comms │ │  comms │
         └────────┘ └────────┘ └────────┘
```

## Run
```bash
# This is a reference implementation. For runnable examples, see meerkat/examples/.
```
