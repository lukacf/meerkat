# 019 — Mob: Pipeline (Rust)

Construct and validate a staged mob definition, spawn a coordinator plus stage
workers, wire their topology, and submit illustrative lint and test turns.
The example is a topology and manual-dispatch walkthrough, not an executing
pass/fail pipeline engine.

## Concepts
- `MobDefinition` profiles, skills, topology, limits, and a sample flow DAG
- Definition validation before mob creation
- Explicit coordinator and stage-worker wiring
- Manual turns sent to the lint and test members
- In-memory mob storage and ephemeral sessions

## Pipeline Stages
```
MobDefinition -> validate -> create mob -> spawn members -> wire topology
                                                        |
                                                        +-> lint turn
                                                        +-> test turn
```

The deploy member is spawned for topology completeness, but this example does
not run a deploy turn, inspect the lint result to gate the test turn, or invoke
the declared `FlowSpec`. Use the flow APIs when application-owned execution and
gating are required.

Members explicitly use `TurnDriven` mode. Each stage uses the shared
`017-mob-coding-swarm-rs/tracked_turn.rs` observer: `start_turn_bounded` admits
the turn and `wait_bounded` observes that exact turn's result. Its answer is
printed before the next stage starts. Neither autonomous inbox admission nor
unrelated lifecycle events prove completion. An observation deadline or typed
turn failure is an error; all members are retired and the mob is shut down on
both success and failure, releasing its supervisor's comms participant.
The lint and test profiles are explicitly externally addressable so these
host-submitted tracked turns are authorized.

A completed lint turn is not treated as proof that lint passed. The printed
sample flow uses an inline JSON schema for its join output, so it needs no
external `schemas/join.json` file.

## Run
```bash
# From the repository root
ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat-mob \
  --example 019-mob-pipeline
```

## Deterministic behavior tests

```bash
./scripts/repo-cargo test -p meerkat-mob --example 019-mob-pipeline
```

The tests execute the same four-member topology and two-stage async body as
`main`, injecting a scripted client through `FactoryAgentBuilder`. They hold
each response pending, check ordering and both printed answers, propagate
failures in either stage, and verify a permanently blocked stage times out
without false completion and leaves no sessions after retirement. No external
transport or credentials are used.
