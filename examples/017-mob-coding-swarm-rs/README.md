# 017 — Mob: Coding Swarm (Rust)

Define a coding-team mob, validate it, spawn one lead and one worker, wire them,
and submit a planning prompt to the lead. The example focuses on embedded mob
construction and lifecycle rather than delegated task execution.

## Concepts

- `MobDefinition` - declarative mob structure (profiles, wiring, skills)
- `MobDefinition::from_toml` - build a mob definition from an inline TOML template
- `MobBuilder` - create or resume a mob
- `MobHandle` - interact with a running mob
- Mob operations - spawn, retire, wire, status

The example intentionally uses `build_ephemeral_service` and in-memory mob
storage. It does not ask the lead to spawn workers or merge worker results, and
it does not demonstrate durable runtime-backed recovery.

The lead explicitly uses `MobRuntimeMode::TurnDriven`: autonomous inbox delivery
does not support this tracked-turn API. Its turn is admitted with
`start_turn_bounded` and its exact committed
answer is awaited with `wait_bounded` before printing it and retiring members.
A 60-second observation deadline or turn failure is an error, not completion;
the deadline does not itself cancel admitted work. Cleanup retires the members.
Mob lifecycle events are displayed only as diagnostics, never as response proof.

## Mob Architecture
```
User prompt -> lead-1 (claude-opus-4-8)
                   |
                   | explicit wire
                   v
              worker-1 (claude-sonnet-4-6)
```

## Run
```bash
# From the repository root
ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat-mob \
  --example 017-mob-coding-swarm
```
