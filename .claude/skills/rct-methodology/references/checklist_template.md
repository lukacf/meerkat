# [Project Name] Implementation Checklist

**Purpose:** Task-oriented checklist for agentic AI execution of [brief description].

**Implementation Plan:** See `[path/to/implementation-plan.md]` for full technical details.

**Specification:** See `[path/to/spec.md]` for authoritative requirements.

---

## RCT Methodology Overview

This implementation follows the **RCT (Representation Contract Tests)** methodology. Key principles:

1. **Gate 0 Must Be Green First** - Representation contracts must pass before any behavior implementation
2. **Representations First, Behavior Second, Internals Last**
3. **Integration Tests Over Unit Tests** - System proof trumps local correctness
4. **No Platonic Completion** - Passing tests are the source of truth, not code quality

### RCT Gate Progression (Methodology Gates)
- **RCT Gate 0**: RCT tests green (round-trip, encoding, NULL semantics)
- **RCT Gate 1**: E2E scenarios written and executable (red OK)
- **RCT Gate 2**: Integration choke-point tests written and executable (red OK)
- **RCT Gate 3**: Unit tests as needed to support integration

> **Note:** These are **methodology gates**, separate from the Phase Gates below. Phase Gates are verification checkpoints at the end of each implementation phase.

---

## Gate Naming Clarification

- **RCT Gates** refer to methodology stages (RCT Gate 0–3) and do NOT align with implementation phases.
- **Phase Gates** are the verification checkpoints at the end of each phase (Phase 0 Gate, Phase 1 Gate, etc.).
- When this checklist says "Phase X Gate Verification," it means the checkpoint for that implementation phase.

---

## Task Format (Agent-Ready)

Each task should be:
- **Atomic**: One deliverable per task
- **Observable**: Clear "done when" condition
- **File-specific**: Explicit output location when applicable

Format: `- [ ] <action> (Done when: <observable condition>)`

---

## Global Dependencies

<!-- List cross-phase dependencies that affect multiple phases -->
- [Dependency 1] must exist before [Phase X] can be implemented
- [Dependency 2] is required by all subsequent phases

---

## Phase Completion Protocol

When completing a phase, use this checklist:

1. [ ] All phase checklist items marked complete
2. [ ] Run relevant tests, note results
3. [ ] Spawn reviewer agents IN PARALLEL (single message, multiple Task calls)
4. [ ] Collect all verdicts
5. [ ] If any BLOCK verdicts:
   - Address blocking issues
   - Re-run *all* reviewers
6. [ ] Record phase completion with reviewer verdicts
7. [ ] Proceed to next phase

---

## Phase 0: Representation Contracts (RCT Gate 0 - MUST BE GREEN)

**Goal:** Lock down all core nouns across boundaries. No behavior yet.

**Dependencies:** None (this is the foundation)

### Tasks - Migration

<!-- One task per table, one per index -->
- [ ] Create migration file `[path]` (Done when: file exists)
- [ ] Add DEFINE TABLE [table_name] (Done when: migration includes definition)

### Tasks - Types

<!-- One task per struct, one per enum -->
- [ ] Create module `[path]` (Done when: module compiles)
- [ ] Implement [StructName] in `[file]` (Done when: compiles with all spec fields)
- [ ] Implement [EnumName] in `[file]` (Done when: enum compiles)

### Tasks - Repositories

<!-- One task per entity -->
- [ ] Create [Entity]Repository with CRUD (Done when: create/get/update/delete tests pass)

### Tasks - RCT Tests

<!-- One task per round-trip test, one per invariant -->
- [ ] Write round-trip test for [Type] (Done when: test_[type]_roundtrip passes)
- [ ] Write invariant test: [invariant] (Done when: test_[invariant] passes)

### Phase 0 Gate Verification Commands

```bash
# Run type tests
cargo test -p [types-crate]

# Run storage tests
cargo test -p [storage-crate] -- --test-threads=1
```

### Phase 0 Gate Review

Spawn reviewers IN PARALLEL:
- **RCT Guardian**: Verify representation round-trips for all new types
- **Spec Auditor**: Verify all core fields present on all tables

---

## Phase N: [Name]

**Goal:** [One sentence describing the phase outcome]

**Dependencies:** [List prerequisite phases and specific components]

### Tasks - [Category]

- [ ] [Atomic action] (Done when: [observable condition])

### Phase N Gate Verification Commands

```bash
cargo test -p [relevant-crate]
```

### Phase N Gate Review

Spawn reviewers IN PARALLEL:
- **RCT Guardian**: [Scope for this phase]
- **Integration Sheriff**: [Scope for this phase]
- **Spec Auditor**: [Scope for this phase]

---

## Final Phase: API + Integration + E2E

**Goal:** Expose all functionality and prove system correctness.

**Dependencies:** All prior phases must be complete

### Tasks - API Endpoints

- [ ] Implement [endpoint] (Done when: API responds correctly)

### Tasks - Scenario Tests

- [ ] Implement S[N].1 [scenario name] (Done when: test verifies [behavior])

### Tasks - Final Verification

- [ ] Run full test suite (Done when: all tests pass)
- [ ] Verify [system property] (Done when: [observable condition])

### Final Phase Gate Verification Commands

```bash
cargo test -p [scenarios-crate]
cargo test -p [api-crate]
cargo integration
cargo t
```

### Final Phase Gate Review

Spawn reviewers IN PARALLEL:
- **RCT Guardian**: Final representation verification (scope: all types)
- **Integration Sheriff**: Verify all choke points covered (scope: all phases)
- **Spec Auditor**: Verify 100% spec coverage (scope: full spec)
- **Code Health**: Production readiness check

---

## Success Criteria

1. [ ] All existing tests still pass
2. [ ] Gate 0 RCT tests green
3. [ ] All scenario tests pass
4. [ ] All integration tests pass
5. [ ] [Domain-specific criterion]
6. [ ] 100% spec coverage verified

---

## Appendix: Reviewer Agent Prompts

<!-- Include the full prompts from references/reviewer_prompts.md -->
<!-- Customize phase-to-spec mapping for Spec Auditor -->
