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
  prompts/        # optional Luka prompts; require a separate complete installation
  scripts/        # optional Luka automation; not supplied in this checkout
  outputs/        # generated artifacts (optional)
```

`.rct/outputs/CHECKLIST.md` is rendered from `.rct/checklist.yaml` and is not the source of truth.

The spec → plan → checklist pipeline is usable manually. This checkout does not
supply the skill's `assets/luka_loop/.rct` payload or the resulting
`.rct/scripts/` automation. Scaffolding, automated validation/review,
loop execution, and automatic finalization require a separately supplied,
verified implementation; do not assume these folders or commands exist.
Preserve existing project metadata rather than overwriting it to fit a template.

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

With Python 3 and PyYAML available, use the bundled standalone renderer from
the target project root for a phases-based checklist matching the template above:

```bash
python3 .claude/skills/rct-methodology/scripts/render_checklist.py
```

In another project, substitute the verified path to the skill's
`scripts/render_checklist.py` and keep the working directory at that project's root.
It renders `.rct/checklist.yaml` → `.rct/outputs/CHECKLIST.md`.
It does not validate arbitrary checklist schemas or run gates.

Use a repo-local `.rct/scripts/render_checklist.py` only if a separate complete
Luka installation actually supplies and verifies it.
