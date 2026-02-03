# Strict Spec → Plan → Checklist (REQUIRED)

This is the required RCT pipeline for 2→3→4:

1) **Spec (ID-driven, immutable once checklist starts)**
   - Use `REQ-###`, `TYPE-###`, `CONTRACT-###`, `INV-###`, `E2E-###` IDs
   - YAML recommended (`spec.yaml`)

2) **Implementation Plan (phase graph + RCT gates)**
   - `plan.yaml` maps deliverables → phases → gates → dependencies

3) **Checklist (machine source of truth)**
   - `checklist.yaml` is canonical
   - Tasks reference spec IDs and include `done_when`

**Conversion rules (strict):**
- Every `REQ` maps to ≥1 checklist task
- Every `TYPE/CONTRACT` maps to Gate‑0 tasks + round‑trip tests
- Every task references a spec ID via `spec_id`
- Task `id` is the task identifier; `spec_id` links to spec (REQ/TYPE/CONTRACT/INV/E2E)
- Every task has a single observable `done_when`
- Every phase defines `verification_commands`

## Required Artifact Locations

Use `.rct/` at repo root:

```
.rct/
  spec.yaml
  plan.yaml
  checklist.yaml
  blockers.yaml   # optional, generated
  agents/         # reviewer prompts (one file per reviewer)
  prompts/        # LUKA_IMPL/LUKA_ROLLBACK/LUKA_FINALIZE
  scripts/        # luka_loop.sh, review_harness.sh, gate_aggregate.py, render_checklist.py
  outputs/        # generated artifacts (optional)
```

`.rct/outputs/CHECKLIST.md` is rendered from `.rct/checklist.yaml` and is not the source of truth.

## Checklist Schema (REQUIRED)

Minimum required fields:

```yaml
project: "Project Name"
last_updated: "YYYY-MM-DD"
phases:
  - id: 0
    title: "Phase Title"
    status: pending | ready_for_gate | approved
    reviewers: [rct-guardian, spec-auditor]
    verification_commands:
      - "cargo test -p my-crate"
    tasks:
      - id: TASK-001
        spec_id: REQ-001
        text: "Do the thing"
        done: false
        done_when: "observable condition"
    gate_results:
      updated_at: "YYYY-MM-DD HH:MM:SS"
      stage: initial | blockers | final
      verdicts:
        - reviewer: rct-guardian
          verdict: APPROVE | BLOCK
          summary: "short note"
```

## Rendering Script

Use:
- `scripts/render_checklist.py`

After scaffolding, use the repo-local `.rct/scripts/render_checklist.py`.
It renders `.rct/checklist.yaml` → `.rct/outputs/CHECKLIST.md`.
