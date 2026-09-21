# Luka Loop Setup Flow (REQUIRED)

Use this flow to prepare a feature's spec, plan, and checklist manually.
Automation is optional: this checkout does not supply the
`assets/luka_loop/.rct` payload or the resulting `.rct/scripts/` validation,
review, loop, and finalization tooling. The bundled scaffold exits
with `Missing assets`. Steps marked conditional require a separately supplied,
complete implementation whose source and prerequisites have been verified.

Preserve existing `.rct/` project metadata. Do not replace an unrelated spec,
plan, or checklist just to enable this workflow.

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

## 4) Scaffold Luka Loop (conditional)
Skip this step in this checkout. Only with a verified complete skill installation
containing `assets/luka_loop/.rct`, invoke its scaffold; these are placeholder
paths for that installation and the target repository:

```bash
python3 /path/to/skills/rct-methodology/scripts/luka_scaffold.py /path/to/repo
```
The scaffold skips existing files by default. Review any copied templates against
the approved spec/plan/checklist; do not use `--force` to overwrite existing
project metadata as a routine setup step.

## 5) Render Human Checklist
The standalone renderer is supplied even without the Luka payload. Require
Python 3, PyYAML in that interpreter, and a phases-based `.rct/checklist.yaml`
matching `checklist_template.md`. From the target project root, if the skill is
at the repository-relative location shown, run:

```bash
python3 .claude/skills/rct-methodology/scripts/render_checklist.py
```
Output at:
```
.rct/outputs/CHECKLIST.md
```
For another project, use the actual verified skill path while keeping the working
directory at that project's root. A repo-local `.rct/scripts/render_checklist.py`
is an alternative only if a separate complete installation actually supplies it.
Rendering is not checklist validation and does not run gates.

## 5.1) Validate Checklist (REQUIRED)
In this checkout, review the checklist against `checklist_template.md` and
`strict_pipeline.md` manually: verify task/spec IDs, phase dependencies,
observable done conditions, reviewers, and verification commands. No repo-local
validator is supplied. Only in a complete installation with a verified validator:

```bash
python3 .rct/scripts/validate_checklist.py
```

## 6) Run Luka Loop (conditional)
Do not offer this command in this checkout. Only after verifying a separately
supplied loop implementation, its prerequisites, and the intended task inputs:

```bash
.rct/scripts/luka_loop.sh
```

## 7) Intended Gate Behavior (conditional)
Verify these design expectations against the supplied implementation rather
than assuming the missing tooling implements them:
- Review cycle: **all → blockers‑only → final‑all**
- Earlier‑phase blocker requires rollback via `origin_phase` + `origin_tasks`

## 8) Finalization (conditional)
No automatic finalization is supplied here. For a complete installation:
- Verify that its finalize prompt runs only after all phases are approved.
- Use the intended commit prefix: `[Luka Loop] <summary>`.
- Commit or push only according to the user's approved policy; do not promise
  automatic publication from this checkout.

## Output to User
After manual setup, or verified optional scaffolding, tell the user:
- where `.rct/` lives and which task files were created or reused
- how to render the checklist, including the schema and PyYAML prerequisites
- whether automation is available; give a loop command only for a verified complete installation
- where to view checklist output
