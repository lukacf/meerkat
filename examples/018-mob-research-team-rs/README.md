# 018 — Mob: Research Team (Rust)

A research team where specialized researchers explore different domains
and a lead analyst synthesizes findings into a cohesive report.

## Concepts
- Diverge/converge coordination pattern
- Multiple specialized profiles (market, tech)
- Role wiring for cross-referencing between researchers
- Evidence-based synthesis

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
