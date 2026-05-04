# 019 — Mob: Pipeline (Rust)

Sequential stage processing where each stage must pass before the next
starts. Models real-world CI/CD, data processing, and approval workflows.

## Concepts
- Sequential stage execution
- Stage handoffs via directed peer requests
- Pass/fail gating between stages
- Artifact passing between stages
- The pipeline prefab

## Pipeline Stages
```
Lint → Test → Security → Deploy
  ↓      ↓       ↓         ↓
 PASS   PASS    PASS      PASS → Success!
 FAIL → Stop pipeline, report failure
```

## Run
```bash
# From the repository root
ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat-mob \
  --example 019-mob-pipeline
```
