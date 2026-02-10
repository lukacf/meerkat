# Skills System Redesign — Implementation Loop

## Your Mission

Implement the Skills System Redesign defined in the TDD implementation plan.
Work through it phase by phase, deliverable by deliverable. Each iteration,
pick up where you left off.

## First: Read State

1. Read `~/.claude/plans/compressed-skipping-spring.md` (the TDD implementation plan — your spec).
2. Read `docs/plans/skills-system-redesign.md` (the architectural spec — your reference for design decisions, wire formats, and contracts).
3. Read `docs/plans/skills-progress.md` (your progress tracker).
   - If it doesn't exist, create it from the plan's phases and deliverables.
4. Read `CLAUDE.md` (project conventions — follow them exactly).

## Progress Tracker Format

Maintain `docs/plans/skills-progress.md` with this structure:

```
# Skills System Redesign — Progress

## Current Phase: 1
## Current Status: implementing | gating | complete

---

## Phase 1: Core Types & Trait Revision

### Tests
- [ ] test_skill_id_collection_extraction
- [ ] test_skill_id_nested_collection
- [ ] test_skill_id_root_level
- [ ] test_skill_id_name_extraction
- [ ] test_skill_filter_default_is_empty
- [ ] test_derive_collections_basic
- [ ] test_derive_collections_nested
- [ ] test_derive_collections_empty
- [ ] test_collection_prefix_match_segment

### Implementation
- [ ] Add SkillFilter, SkillCollection, ResolvedSkill types
- [ ] Add collection() and name() methods on SkillId
- [ ] Add metadata + source_name to SkillDescriptor
- [ ] Add derive_collections() with segment-aware prefix matching
- [ ] Change SkillSource::list() signature to list(&self, filter: &SkillFilter)
- [ ] Add default collections() method on SkillSource
- [ ] Revise SkillEngine trait

## Phase 2: Source Updates
...

---

## Gate Results

### Phase 1 — Attempt 1
- build-gate: ?
- test-gate: ?
- performance-gate: ?
- rust-quality-gate: ?
```

Update this file EVERY iteration before you do anything else. Check off what
you completed last iteration. Then continue.

## TDD Rules

### Per-Phase Workflow
1. Read the plan's specification for the current phase.
2. **Write tests FIRST.** Every test listed in the plan must be written before
   the corresponding implementation code. Tests should initially fail (or not
   compile) — that's correct.
3. **Implement** until all tests pass. Follow the implementation steps in order.
4. Verify: `cargo rct` must pass with zero failures.
5. Check off completed items in the progress tracker.
6. When all tests and implementation items in the phase are checked off,
   transition to gating.

### Implementation Quality
- Implement EXACTLY as specified in the architectural spec. No shortcuts, no stubs, no "will add later."
- Follow Rust Design Guidelines from CLAUDE.md (typed enums, Result over silent failures, zero-allocation iterators, no .unwrap() in library code).
- Reference `docs/plans/skills-system-redesign.md` for exact type definitions, wire formats, escaping rules, cache contracts, etc.

### When All Phase Deliverables Are Checked Off
Transition to gating mode. Set `Current Status: gating`. Run ALL gates in sequence:

#### Gate 1: Build
```bash
cargo build --workspace 2>&1
```
Must compile with zero errors and zero warnings.

#### Gate 2: Tests
```bash
cargo rct 2>&1
```
All fast tests must pass (unit + integration-fast).

#### Gate 3: Performance
Verify build+test time hasn't exploded. Check for:
- Tests that call real APIs (look for API key usage in non-ignored tests)
- Tests with sleep/delay over 1 second
- New tests that spawn external processes unnecessarily
Build time > 180s or test time > 15s for fast tests = FAIL.

#### Gate 4: Spec Accuracy
Launch the spec-accuracy-gate agent (via Task tool) with:
> Review Phase {N} of the skills system redesign.
> Plan file: ~/.claude/plans/compressed-skipping-spring.md
> Architectural spec: docs/plans/skills-system-redesign.md
Every test listed in the plan must exist. Every implementation item must be
fulfilled. Any FAIL = fix and re-gate.

#### Gate 5: Rust Quality
Launch the rust-quality-gate agent (via Task tool) with:
> Review all new and modified files in the current phase.
> Focus on code added/changed since the last phase commit.
Any violation = fix and re-gate.

### Gate Failure Procedure
1. Record the failure in the progress tracker under Gate Results.
2. Fix the issue(s) identified by the failing gate.
3. Re-run ALL gates from the beginning (a fix for one gate can break another).
4. Increment the attempt counter.
5. Repeat until all 5 gates pass on the same attempt.

### When All Gates Pass
1. Record PASS for all gates in the progress tracker.
2. Stage all changed files and create a commit:
   ```
   Phase {N}: {phase title}

   {2-3 sentence summary of what was implemented}
   ```
3. Set `Current Phase: {N+1}` and `Current Status: implementing`.
4. If this was Phase 12, output the completion promise (see below).

## What NOT To Do

- Do NOT implement multiple phases in one iteration.
- Do NOT skip phases or reorder them (they have dependencies).
- Do NOT mark a deliverable as done if it has TODO/stub/placeholder code.
- Do NOT run gates until ALL deliverables in the phase are checked off.
- Do NOT commit partial phases.
- Do NOT add features, refactoring, or "improvements" not in the plan.
- Do NOT modify the plan files. They are immutable. If you think the plan is
  wrong, note the concern in the progress tracker and implement it anyway.
  If it is blocking, ask the user for a judgement. Note it in the progress tracker.
- Do NOT write tests that call live APIs (HTTP, git remotes) without `#[ignore]`.
  Use InMemorySkillSource, temp dirs, and mock servers for all fast tests.

## Phase Dependencies

```
Phase 1: Core Types          ← foundation
Phase 2: Source Updates       ← depends on Phase 1 (new trait signatures)
Phase 3: Renderer             ← depends on Phase 1 (new types)
Phase 4: Engine Revision      ← depends on Phase 1, 2, 3
Phase 5: Configuration        ← depends on Phase 1
Phase 6: Factory Wiring       ← depends on Phase 4, 5
Phase 7: Per-Turn Activation  ← depends on Phase 4, 6
Phase 8: Discovery Tools      ← depends on Phase 4, 6
Phase 9: Wire Format          ← depends on Phase 6
Phase 10: HttpSkillSource     ← depends on Phase 2, 5
Phase 11: GitSkillSource      ← depends on Phase 2
Phase 12: Config Resolution   ← depends on Phase 5, 10, 11
```

## Completion

When Phase 12 passes all gates and is committed, output:

<promise>SKILLS_REDESIGN_COMPLETE</promise>

Until then, keep iterating. There is always more work to do.
