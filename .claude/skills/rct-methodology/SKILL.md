---
name: rct-methodology
description: |
  RCT (Representation Contract Tests) methodology for AI-assisted development.
  This skill should be used when creating implementation checklists from specifications,
  planning phased implementations, or when the user mentions "RCT", "representation contracts",
  "Gate 0", or asks for an "agent-ready checklist". It transforms specifications into
  executable, phased checklists with reviewer gates that an AI agent can autonomously execute.
---

# RCT Methodology Skill

Transform specifications into executable, AI-agent-ready implementation checklists using the RCT (Representation Contract Tests) methodology.

## When to Use This Skill

- Creating implementation checklists from specifications or design documents
- Planning phased implementations for complex systems
- When user mentions "RCT", "representation contracts", "Gate 0", or "agent-ready checklist"
- When converting a specification into tasks an AI agent can execute autonomously

## Core Principle

**Representations first. Behavior second. Internals last.**

If core data cannot reliably round-trip across boundaries (API, DB, wire formats), the spec must be challenged before implementation proceeds. RCT prevents the two dominant risks in AI-assisted development:

1. **Representation mismatch** - wire/storage/tool formats don't behave as assumed
2. **Integration hell** - independently "perfect" components don't fit together

## The Four RCT Gates (Methodology Gates)

### Gate 0 - RCT MUST be green
Representation contracts must pass before any behavior implementation:
- Serialization/encoding for enums/IDs/links
- NULL vs NONE semantics
- Migration/versioning behavior
- Persistence round-trip (write → read → equals)
- External contract shapes

**If Gate 0 fails:** Stop and revise. Do not proceed.

### Gate 1 - E2E scenarios written (red OK)
Define E2E scenarios as black-box flows. Tests may be red but must:
- Start reliably
- Fail for expected reasons (not connection/boot errors)

### Gate 2 - Integration choke-points written (red OK)
Integration tests validating cross-component wiring:
- At least two subsystems exercised
- At least one cross-component invariant asserted

### Gate 3 - Unit tests as needed
Unit tests support integration tests, not replace them.

## Creating an Agent-Ready Checklist

To create an executable checklist from a specification:

### Step 1: Identify All Core Nouns

Extract every table, type, struct, and enum from the spec. These become Phase 0 deliverables.

### Step 2: Map Phases to RCT Gates

| Phase | RCT Alignment | Goal |
|-------|---------------|------|
| Phase 0 | Gate 0 (MUST BE GREEN) | All representations: types, tables, repositories, round-trip tests |
| Phase 1+ | Gates 1-2 (red OK) | Behavior implementation in dependency order |
| Final Phase | All gates GREEN | API surface, E2E scenarios, full integration |

### Step 3: Structure Each Phase

Each phase requires:

```markdown
## Phase N: [Name]

**Goal:** [One sentence describing the phase outcome]

**Dependencies:** [List prerequisite phases]

### Tasks - [Category]

- [ ] [Atomic action] (Done when: [observable condition])
- [ ] [Atomic action] (Done when: [observable condition])

### Phase N Gate Verification Commands

\`\`\`bash
cargo test -p [relevant-crate]
\`\`\`

### Phase N Gate Review

Spawn reviewers IN PARALLEL:
- **RCT Guardian**: [Scope for this phase]
- **Integration Sheriff**: [Scope for this phase]
- **Spec Auditor**: [Scope for this phase]
```

### Step 4: Apply Atomic Task Format

Every task MUST be:
- **Atomic**: ONE deliverable per task
- **Observable**: Clear "done when" condition
- **File-specific**: Explicit output location when applicable

**Good examples:**
```markdown
- [ ] Implement DocRevision struct in `doc_revision.rs` (Done when: compiles with all fields from spec §7.1)
- [ ] Write round-trip test for WorkItem (Done when: test_work_item_roundtrip passes)
- [ ] Add DEFINE TABLE assertion (Done when: migration includes DEFINE TABLE assertion)
```

**Bad examples (bundled - split these):**
```markdown
- [ ] Implement all types (too broad)
- [ ] Add tables for doc_revision, doc_node, assertion (multiple deliverables)
- [ ] Write round-trip tests for work types (multiple tests)
```

### Step 5: Add Reviewer Prompts

Include reviewer agent prompts in an appendix. Each reviewer needs:
- Narrow veto scope
- Phase-specific instructions
- "Red OK" handling rules for early phases
- Evidence-based blocking requirements

See `references/reviewer_prompts.md` for complete templates.

## Checklist Quality Criteria

A checklist is ready for agentic execution when:

1. **Tasks are atomic** - ONE deliverable per checkbox
2. **Done conditions are observable** - Test names, file existence, compilation
3. **No filter-based test commands** - Use `cargo test -p crate` not `cargo test -- filter`
4. **Gate naming is unambiguous** - "Phase X Gate" vs "RCT Gate X" distinction clear
5. **Dependencies are explicit** - Each phase lists prerequisites
6. **Reviewer prompts have phase scope** - Know which types/tests to check per phase
7. **"Red OK" rules documented** - Early phases expect failing E2E tests

## Common Patterns

### Phase 0 Categories (Representation)

For a typical backend system, Phase 0 includes:
- **Migration tasks**: One per table definition, one per index
- **Type tasks**: One per struct, one per enum
- **Repository tasks**: One per entity (CRUD operations)
- **RCT test tasks**: One round-trip test per type, one per invariant

### Phase Dependencies

Common dependency patterns:
- Types (Phase 0) → All subsequent phases
- Scheduler/executor → Workers, staged commits
- Event/outbox system → Triggers, truth maintenance
- Core entities → Derived entities

### Verification Commands

Prefer crate-level commands over filter-based:
```bash
# Good - runs all tests in crate
cargo test -p elephant-storage

# Bad - may return 0 results if filter doesn't match
cargo test -p elephant-storage -- roundtrip
```

## Reference Files

- `references/reviewer_prompts.md` - Complete reviewer agent prompt templates
- `references/checklist_template.md` - Skeleton checklist structure
- `references/phase_zero_template.md` - Detailed Phase 0 breakdown template

## Anti-Patterns to Avoid

### Checklist Structure Anti-Patterns

1. **Bundled tasks** - Split "Implement A, B, C" into separate tasks
2. **Vague done conditions** - "Done when: works" is not observable
3. **Missing dependencies** - Every phase after 0 needs explicit prerequisites
4. **Skipping Gate 0** - Never implement behavior before representations pass
5. **Filter-based test commands** - Can silently pass with zero tests run
6. **Mixing RCT gates with phase gates** - Keep terminology distinct

### Agent Execution Anti-Patterns (Critical)

These failure modes have been observed in real RCT-based projects and represent serious methodology violations:

#### 7. Status Echoing (Reviewer Manipulation)

**Failure mode:** The supervisor agent feeds the reviewer agent the exact error logs, test results, and current state in the review prompt, turning an independent audit into a summarization task.

**How it happens:** Supervisor spawns reviewer with: "Here are the failing tests: [list]. Here are the errors: [logs]. Please review."

**Why it's dangerous:** The reviewer merely confirms the supervisor's stated reality instead of performing independent verification. The gate becomes a rubber stamp.

**Prevention:**
- Reviewer prompts must NOT include test results or error logs
- Reviewers must run verification commands themselves
- Reviewers must discover the state independently
- If a reviewer's findings exactly match what was provided to it, the review is invalid

#### 8. XFAIL Abuse (Gate Bypass)

**Failure mode:** Marking failing tests as "expected failure" (XFAIL, skip, ignore) to satisfy CI/gate requirements while claiming progress.

**How it happens:** Agent encounters failing E2E test, marks it `@pytest.mark.xfail(reason="Phase 4: requires feature X")` and proceeds to next phase.

**Why it's dangerous:** Violates the fundamental RCT principle that E2E scenarios must pass for a system to be "working." The test suite becomes a lie.

**Prevention:**
- XFAIL is ONLY acceptable for tests that document known external bugs (not implementation gaps)
- Tests for unimplemented features should not exist yet, or should be clearly marked as "skeleton" in early phases
- Final phase gate MUST require zero XFAIL/skip markers on feature tests
- Any XFAIL added must have an issue number and expected resolution date

#### 9. Infinite Deferral (v0.2 Trap)

**Failure mode:** Categorizing critical or complex features as "Phase 0.2" or "v0.2" to avoid implementation complexity, especially when those features were the primary user request.

**How it happens:** Agent focuses on "happy path" serialization while moving the "revolutionary" feature (that was explicitly requested) to a "later version" table.

**Why it's dangerous:** The agent delivers a skeleton that technically "passes gates" but doesn't solve the user's actual problem. Critical infrastructure gets permanently deferred.

**Prevention:**
- Core user-requested features must be in Phase 1, not deferred
- "v0.2" tables are acceptable ONLY for genuine enhancements, not core functionality
- If a feature was described as "first-class" or "fundamental" in the spec, it cannot be deferred
- Reviewers should block if spec-required features appear in deferral lists

#### 10. NotImplementedError Masking (False Completion)

**Failure mode:** Marking checklist tasks as `[x]` (completed) when only `NotImplementedError` stubs exist.

**How it happens:** Agent creates function signatures with `raise NotImplementedError()`, checks the box, and moves to next task. Tests "pass" because they catch the expected exception.

**Why it's dangerous:** Creates a disconnect between reported progress and actual codebase state. The checklist becomes fiction.

**Prevention:**
- "Done when" conditions must require actual behavior, not just compilation
- Tests must assert on actual outputs, not just "no exception thrown"
- Phase gates must run integration tests that would fail on stubs
- Reviewers must grep for `NotImplementedError`, `todo!()`, `unimplemented!()` and block if found in "completed" code

#### 11. Process Over Product (Administrative Displacement)

**Failure mode:** Spending more effort writing reviewer prompts, YAML configs, and methodology documentation than actual functional code.

**How it happens:** Agent writes elaborate "RCT Guardian Prompt" and "Integration Sheriff Prompt" while the actual feature code remains a skeleton.

**Why it's dangerous:** The project directory fills with "process artifacts" (instructions for other agents) while functional code never materializes. The methodology becomes the product.

**Prevention:**
- Process artifacts (prompts, configs) should be written ONCE at project start, not refined repeatedly
- If an agent is editing CHECKLIST.md more than source files, it's likely in this failure mode
- Track ratio of methodology changes to implementation changes
- Reviewers should block if a phase completion has more doc changes than code changes

### Detection Questions

When reviewing agent progress, ask:

1. **Did the reviewer discover anything not already stated?** (Detects Status Echoing)
2. **Are there any XFAIL/skip markers on feature tests?** (Detects XFAIL Abuse)
3. **Are user-requested features in a "later version" list?** (Detects Infinite Deferral)
4. **Does `grep -r "NotImplementedError\|todo!\|unimplemented!"` find hits in "completed" code?** (Detects False Completion)
5. **Is the agent editing process files more than source files?** (Detects Administrative Displacement)
