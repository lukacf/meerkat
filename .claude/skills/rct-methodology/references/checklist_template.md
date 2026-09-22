# Checklist Template (.rct/checklist.yaml)

This file describes the REQUIRED structure for the YAML checklist.

## Minimal Template

```yaml
project: "Project Name"
last_updated: "YYYY-MM-DD"
phases:
  - id: 0
    title: "Representation Contracts"
    status: pending
    reviewers: [rct-guardian, spec-auditor]
    verification_commands:
      - "cargo test -p types-crate"
      - "cargo test -p storage-crate -- --test-threads=1"
    tasks:
      - id: REQ-001
        spec_id: REQ-001
        text: "Create migration for doc_revision"
        done: false
        done_when: "migration file exists with DEFINE TABLE doc_revision"
      - id: TYPE-001
        spec_id: TYPE-001
        text: "Implement DocRevision struct"
        done: false
        done_when: "DocRevision compiles with all spec fields"
    gate_results:
      updated_at: ""
      stage: ""
      verdicts: []
```

## Field Notes

- `status` must be one of: `pending`, `ready_for_gate`, `approved`
- `tasks.done_when` must be a single, observable condition
- `tasks.spec_id` must reference a spec ID (REQ/TYPE/CONTRACT/INV/E2E)
- `reviewers` list is required for every phase
- `verification_commands` list is required for every phase
- `gate_results` records reviewer results; a separately supplied Luka gate aggregator may populate it automatically, but no aggregator is supplied in this checkout

## Rendering

The standalone renderer is supplied; the Luka `assets/luka_loop/.rct` payload
and repo-local `.rct/scripts/` automation are not. With Python 3 and PyYAML
available, render a checklist using the phases-based structure above from the
project root:

```bash
python3 .claude/skills/rct-methodology/scripts/render_checklist.py
```

This path assumes the skill is bundled at that repository-relative location.
For another project, use the actual verified skill path while keeping the
working directory at the target project's root. Do not overwrite unrelated
existing `.rct/` metadata to fit this template. Rendering does not validate
the checklist or execute gates.

The human-readable output is:

```
.rct/outputs/CHECKLIST.md
```

Use `.rct/scripts/render_checklist.py` only if a separately supplied complete
Luka installation provides and verifies that script.
