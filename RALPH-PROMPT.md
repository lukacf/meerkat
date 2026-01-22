# Ralph Loop Prompt for meerkat-comms

## Usage

```bash
/ralph-loop "[paste prompt below]" --completion-promise "COMPLETE" --max-iterations 50
```

---

## Prompt

```
Implement meerkat-comms following CHECKLIST-COMMS.md using RCT methodology.

## Your Mission

Complete ONE PHASE per iteration. Do not proceed to the next phase until all reviewers APPROVE the current phase.

## State Detection (Do This First)

1. Read CHECKLIST-COMMS.md
2. Find the first phase with unchecked tasks - that's your current phase
3. If all phases complete and Final Phase reviewers approved, output <promise>COMPLETE</promise>

## Phase Execution Loop

For your current phase:

### Step 1: Implement Tasks
- Work through each unchecked task in order
- Mark tasks [x] as you complete them
- Follow "Done when" conditions exactly

### Step 2: Run Gate Verification
- Run the phase's verification commands (cargo test)
- All tests must pass before proceeding to review

### Step 3: Spawn Reviewers IN PARALLEL
Based on the phase, spawn the appropriate reviewers as parallel Task agents:
- Use the prompts from CHECKLIST-COMMS.md Appendix
- Fill in {PHASE} and {PHASE_SCOPE} placeholders
- Spawn them in a SINGLE message with multiple Task tool calls

### Step 4: Evaluate Verdicts
- If ALL reviewers return APPROVE: Mark phase complete, stop this iteration
- If ANY reviewer returns BLOCK:
  - Address the blocking issues they identified
  - Do NOT proceed to next phase
  - The next iteration will re-run this phase's review

## Critical Rules

1. ONE PHASE PER ITERATION - Never start a new phase in the same iteration
2. REVIEWERS MUST APPROVE - 100% approval required before phase is complete
3. NO SKIPPING REVIEWS - Every phase must have its reviewers run
4. REVIEWERS ARE INDEPENDENT - Do not feed them test results or state
5. MARK PROGRESS - Update CHECKLIST-COMMS.md checkboxes as you work

## Completion

Output <promise>COMPLETE</promise> ONLY when:
- All phases through Final Phase have all tasks checked [x]
- Final Phase reviewers (RCT Guardian, Integration Sheriff, Spec Auditor, Methodology Integrity) ALL returned APPROVE
- `cargo test --workspace` passes

## If Stuck

If same phase fails review 3+ times:
1. Document the blocking issues in a comment in CHECKLIST-COMMS.md
2. Try a different approach
3. If still stuck after 5 attempts on same phase, create BLOCKERS.md with details

Do NOT output <promise>COMPLETE</promise> if stuck - let the iteration limit handle it.
```

---

## Shorter Version (if context is tight)

```
Implement meerkat-comms per CHECKLIST-COMMS.md.

ONE PHASE PER ITERATION:
1. Read checklist, find first phase with unchecked tasks
2. Complete all tasks, mark [x], run gate verification
3. Spawn phase reviewers IN PARALLEL (from Appendix prompts)
4. If ALL APPROVE: phase done, stop iteration
5. If ANY BLOCK: fix issues, next iteration retries

Output <promise>COMPLETE</promise> only when:
- All phases done with [x] marks
- Final Phase reviewers ALL APPROVE
- cargo test --workspace passes

Never skip reviews. Never proceed without 100% approval.
```

---

## Notes

- **Iteration limit**: 50 should be enough for 8 phases with some retries
- **Cost estimate**: ~$20-50 depending on how many review cycles needed
- **Monitor**: Check CHECKLIST-COMMS.md periodically to see progress
- **Cancel**: Use `/cancel-ralph` if it goes off track
