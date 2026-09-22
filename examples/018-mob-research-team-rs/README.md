# 018 — Mob: Research Team (Rust)

Define and validate a research-team mob, spawn a lead plus two specialists,
wire the team, and submit one question-generation prompt to the lead.

## Concepts

- Diverge/converge roles encoded in the definition and skills
- Multiple specialized profiles (market, tech)
- Role wiring for cross-referencing between researchers
- Inline role instructions for evidence and synthesis

The runnable path does not send research turns to the specialists or synthesize
their outputs. It demonstrates the roster and topology that an application can
use to implement that workflow.

The example uses the standalone in-memory mob and session path. Use a
runtime-backed surface when the mob must survive process restarts or accept
durable external work.

The lead explicitly uses `MobRuntimeMode::TurnDriven`, the lane supported by
the tracked-turn API (not autonomous inbox delivery). Its exact tracked turn
is awaited and its answer printed before member
retirement (shared helper in `../017-mob-coding-swarm-rs/tracked_turn.rs`).
Failure or a 60-second observation deadline is reported as an error; timeout
does not prove cancellation or completion. Cleanup retires members. Historical
mob lifecycle events do not satisfy the response wait.

## Profiles
| Profile | Model | Role |
|---------|-------|------|
| lead-analyst | claude-opus-4-8 | Coordinates research, synthesizes findings |
| market-researcher | claude-sonnet-4-6 | Competitive analysis, market sizing |
| tech-researcher | claude-sonnet-4-6 | Technical feasibility |

## Run
```bash
# From the repository root
ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat-mob \
  --example 018-mob-research-team
```
