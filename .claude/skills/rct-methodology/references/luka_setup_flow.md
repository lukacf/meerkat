# Luka Loop Setup Flow (REQUIRED)

This is the required agentic flow to go from a feature request to a runnable Luka Loop.

## 0) Intake / Discovery (ask first)
Collect minimal structured context using multiple‑choice where possible.

**Suggested questions (multiple choice):**
1. **Project type**: backend | frontend | full‑stack | data/ML | infra
2. **Primary language**: Rust | TS/JS | Python | Go | Other
3. **Primary storage**: SQL | NoSQL | File | External API | None
4. **External contracts**: REST | GraphQL | gRPC | CLI | None
5. **Testing split**: unit/integration only | includes E2E | unknown
6. **Default reviewers**: accept defaults | customize per phase
7. **Commit policy**: allow auto‑commit/push | require confirmation

If user provides a spec/plan/checklist already, skip to the relevant step and validate.

## 1) Build Spec (`.rct/spec.yaml`)
Produce a strict, ID‑driven spec. Require IDs for:
- REQ‑###
- TYPE‑###
- CONTRACT‑###
- INV‑###
- E2E‑###

Confirm with user before proceeding.

## 2) Build Implementation Plan (`.rct/plan.yaml`)
Map deliverables to phases and RCT gates. Include dependencies.

Confirm with user before proceeding.

## 3) Build Checklist (`.rct/checklist.yaml`)
- Tasks are atomic, each references a spec ID.
- Each task has a single observable `done_when`.
- Each phase lists reviewers (defaults + project‑specific additions).
- Each phase includes `verification_commands` for reviewers.

Confirm with user before proceeding.

## 4) Scaffold Luka Loop
Run:
```
python <CODEX_HOME>/skills/rct-methodology/scripts/luka_scaffold.py <repo>
```
Then copy generated YAML files into `.rct/` or overwrite if `--force`.

## 5) Render Human Checklist
Run:
```
.rct/scripts/render_checklist.py
```
Output at:
```
.rct/outputs/CHECKLIST.md
```
Use the repo-local `.rct/scripts/*` after scaffolding (not the skill-level scripts).

## 5.1) Validate Checklist (REQUIRED)
Run:
```
.rct/scripts/validate_checklist.py
```

## 6) Run Luka Loop
```
.rct/scripts/luka_loop.sh
```

## 7) Gate Behavior (required)
- Review cycle: **all → blockers‑only → final‑all**
- Earlier‑phase blocker requires rollback via `origin_phase` + `origin_tasks`

## 8) Finalization
- Finalize prompt runs after all phases approved.
- Commit prefix: `[Luka Loop] <summary>`
- Push to current branch.

## Output to User
After scaffolding, tell the user:
- where `.rct/` lives
- how to render checklist
- how to run the loop
- where to view checklist output
