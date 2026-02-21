# 018 — Mob: Research Team (Rust)

A research team where specialized researchers explore different domains
and a lead analyst synthesizes findings into a cohesive report.

## Concepts
- Diverge/converge coordination pattern
- Multiple specialized profiles (market, tech, user research)
- Role wiring for cross-referencing between researchers
- Task board for tracking research hypotheses
- Evidence-based synthesis

## Profiles
| Profile | Model | Role |
|---------|-------|------|
| lead-analyst | claude-opus-4-6 | Coordinates research, synthesizes findings |
| market-researcher | claude-sonnet-4-5 | Competitive analysis, market sizing |
| tech-researcher | claude-sonnet-4-5 | Technical feasibility |
| user-researcher | claude-sonnet-4-5 | Personas, pain points |

## Run
```bash
ANTHROPIC_API_KEY=sk-... cargo run --example 018_mob_research_team
```
