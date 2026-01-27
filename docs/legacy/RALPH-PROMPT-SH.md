# Ralph Loop Prompt for meerkat-shell

## Usage

```bash
/ralph-loop "[paste prompt below]" --completion-promise "COMPLETE" --max-iterations 50
```

---

## Prompt

```
Implement meerkat-shell following CHECKLIST-SH.md using RCT methodology.

## Your Mission

Complete ONE PHASE per iteration. You are the ORCHESTRATOR - you spawn swarms for implementation and reviewers for verification. Do not implement code yourself.

## Key Documents

- Specification: SH_DESIGN.md (authoritative requirements)
- Checklist: CHECKLIST-SH.md (phase tasks and gate reviews)

## State Detection (Do This First)

1. Read CHECKLIST-SH.md
2. Find the current phase using this logic:
   - Look for approval markers: PHASE_N_APPROVED
   - The current phase is the FIRST phase WITHOUT an approval marker
   - If a phase has all [x] but NO approval marker, it needs review (not the next phase!)
3. If all phases including Phase 7 have approval markers, output <promise>COMPLETE</promise>

## Phase Execution Loop

For your current phase:

### Step 1: Check Phase State
- If phase has unchecked tasks: spawn implementation swarm (Step 2)
- If phase has all [x] but no approval marker: skip to Step 3 (review)

### Step 2: Spawn Implementation Swarm

Create a team to implement the phase tasks using TDD:

1. Create team using Teammate tool with spawnTeam operation
2. Create tasks using TaskCreate for each unchecked item in the phase
3. Spawn Opus teammates using Task tool with:
   - subagent_type: general-purpose
   - model: opus
   - team_name: <your-team>
   - One teammate per 3-4 tasks (balance parallelism vs coordination)

TDD Implementation Prompt for Teammates:

   You are implementing Phase {N} of meerkat-shell per CHECKLIST-SH.md.

   Methodology: TDD (Test-Driven Development)

   For EACH task:
   1. WRITE TEST FIRST - Create failing test that validates Done-when condition
   2. RUN TEST - Verify it fails for the right reason
   3. IMPLEMENT - Write minimal code to make test pass
   4. REFACTOR - Clean up while keeping tests green

   Your Tasks: [List assigned tasks from checklist]

   Key Files:
   - Spec: SH_DESIGN.md (authoritative)
   - Types go in: meerkat-tools/src/builtin/shell/types.rs
   - Tests go in: same file as implementation (#[cfg(test)] mod tests)

   Rules:
   - Tests MUST exist before implementation
   - Mark task [x] only after test passes
   - No todo!() or unimplemented!() - real implementations only
   - Follow spec exactly - check SH_DESIGN.md for schemas/formats

   When done with your tasks, mark them complete and notify team lead.

4. Wait for swarm completion - monitor task list until all tasks complete
5. Run gate verification commands from checklist
6. Mark checkboxes [x] in CHECKLIST-SH.md for completed tasks
7. Cleanup team using Teammate tool cleanup operation

### Step 3: Spawn Reviewers IN PARALLEL

Based on the phase, spawn the appropriate reviewers as parallel Task agents:
- Use reviewers listed in CHECKLIST-SH.md for this phase
- Pass prompt: Review Phase {N} of meerkat-shell
- Spawn them in a SINGLE message with multiple Task tool calls
- Do NOT include test results or state in reviewer prompts

### Step 4: Evaluate Verdicts

- If ALL reviewers return APPROVE:
  - Add approval marker after the phase header: PHASE_N_APPROVED
  - Stop this iteration
- If ANY reviewer returns BLOCK:
  - Uncheck [x] to [ ] the task(s) related to the blocking issue
  - Note: Next iteration will re-spawn swarm to fix issues
  - Do NOT add approval marker
  - Do NOT proceed to next phase

## Critical Rules

1. ONE PHASE PER ITERATION - Never start a new phase in the same iteration
2. ORCHESTRATOR ONLY - You spawn swarms, you do not write code yourself
3. TDD MANDATORY - Swarm teammates must write tests first
4. OPUS MODEL - All implementation teammates use model: opus
5. APPROVAL MARKER = DONE - A phase is only complete when it has PHASE_N_APPROVED
6. CHECKBOXES != APPROVAL - All [x] without approval marker means review still needed
7. REVIEWERS ARE INDEPENDENT - Do not feed them test results or state
8. CLEANUP TEAMS - Always cleanup team after phase (success or failure)

## Completion

Output <promise>COMPLETE</promise> ONLY when:
- All phases 0-7 have approval markers
- cargo test --workspace passes
- cargo clippy --workspace -- -D warnings passes

## If Stuck

If same phase fails review 3+ times:
1. Document the blocking issues in a comment in CHECKLIST-SH.md
2. Spawn swarm with explicit instructions to address specific failures
3. If still stuck after 5 attempts on same phase, create BLOCKERS-SH.md with details

Do NOT output <promise>COMPLETE</promise> if stuck - let the iteration limit handle it.
```

---

## Shorter Version (if context is tight)

```
Implement meerkat-shell per CHECKLIST-SH.md. You are ORCHESTRATOR only.

ONE PHASE PER ITERATION:
1. Read checklist, find first phase WITHOUT PHASE_N_APPROVED marker
2. If unchecked tasks: spawn Opus swarm (TDD methodology, model: opus)
3. Wait for swarm, run gate commands, mark [x], cleanup team
4. Spawn phase reviewers IN PARALLEL (do not feed them state)
5. If ALL APPROVE: add PHASE_N_APPROVED marker, stop iteration
6. If ANY BLOCK: uncheck failing task(s), next iteration re-spawns swarm to fix

TDD for swarm: Write test -> Run (fails) -> Implement -> Run (passes) -> Refactor

CRITICAL:
- You orchestrate, swarm implements
- All teammates use opus model
- Checkboxes [x] != phase complete. Only approval marker = complete.

Output <promise>COMPLETE</promise> only when:
- All phases 0-7 have approval markers
- cargo test --workspace passes

Never skip reviews. Never proceed without approval marker.
```

---

## Notes

- **Iteration limit**: 50 should be enough for 8 phases with some retries
- **Cost estimate**: Higher due to Opus swarms (~$50-150 depending on complexity)
- **Monitor**: Check CHECKLIST-SH.md for `PHASE_N_APPROVED` markers
- **Cancel**: Use `/ralph-loop:cancel-ralph` if it goes off track
- **Team names**: Use pattern `sh-phase-{N}` for easy identification

## Approval Marker Format

After Phase N reviewers approve, add this line immediately after the phase header:

```markdown
## Phase N: [Name]
PHASE_N_APPROVED
```

This marker is the source of truth for phase completion, not the checkboxes.

## TDD Enforcement

The swarm MUST follow TDD. Signs of TDD violation (reviewers should catch):
- Implementation exists without corresponding test
- Tests added after implementation (check git history if needed)
- `todo!()` or `unimplemented!()` in completed code
- Tests that do not actually verify the Done-when condition

If TDD is violated, reviewer should BLOCK and swarm must redo the task properly.
