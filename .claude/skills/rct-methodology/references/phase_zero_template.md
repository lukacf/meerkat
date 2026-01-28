# Phase 0 Template (.rct/checklist.yaml snippet)

Use this as a Phase 0 block in `.rct/checklist.yaml`. Phase 0 corresponds to RCT Gate 0 and MUST be green before behavior work.

```yaml
- id: 0
  title: "Representation Contracts"
  status: pending
  reviewers: [rct-guardian, spec-auditor]
  verification_commands:
    - "cargo test -p types-crate"
    - "cargo test -p storage-crate -- --test-threads=1"
  tasks:
    # Migration tasks (one per table)
    - id: TYPE-001
      spec_id: TYPE-001
      text: "Add DEFINE TABLE doc_revision"
      done: false
      done_when: "migration includes DEFINE TABLE doc_revision"
    # Type tasks (one per struct/enum)
    - id: TYPE-002
      spec_id: TYPE-002
      text: "Implement DocRevision struct"
      done: false
      done_when: "DocRevision compiles with all spec fields"
    # Repository tasks (one per entity)
    - id: TYPE-003
      spec_id: TYPE-003
      text: "Add DocRevisionRepository CRUD"
      done: false
      done_when: "CRUD round-trip tests pass"
    # RCT tests (one per representation)
    - id: TEST-001
      spec_id: TEST-001
      text: "Round-trip test for DocRevision"
      done: false
      done_when: "test_doc_revision_roundtrip passes"
  gate_results:
    updated_at: ""
    stage: ""
    verdicts: []
```

Notes:
- Every task must reference a spec ID (REQ/TYPE/CONTRACT/INV/E2E).
- Use `done_when` with a single observable condition.
