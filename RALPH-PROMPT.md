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
2. Find the current phase using this logic:
   - Look for approval markers: "<!-- Phase N: APPROVED -->"
   - The current phase is the FIRST phase WITHOUT an approval marker
   - If a phase has all [x] but NO approval marker, it needs review (not the next phase!)
3. If all phases including Final Phase have approval markers, output <promise>COMPLETE</promise>

## Phase Execution Loop

For your current phase:

### Step 1: Check Phase State
- If phase has unchecked tasks: implement them
- If phase has all [x] but no approval marker: skip to Step 3 (review)

### Step 2: Implement Tasks
- Work through each unchecked task in order
- Mark tasks [x] as you complete them
- Follow "Done when" conditions exactly
- Run gate verification commands (cargo test)
- All tests must pass before proceeding to review

### Step 3: Spawn Reviewers IN PARALLEL
Based on the phase, spawn the appropriate reviewers as parallel Task agents:
- Use the prompts from CHECKLIST-COMMS.md Appendix
- Fill in {PHASE} and {PHASE_SCOPE} placeholders
- Spawn them in a SINGLE message with multiple Task tool calls

### Step 4: Evaluate Verdicts
- If ALL reviewers return APPROVE:
  - Add approval marker after the phase header: "<!-- Phase N: APPROVED -->"
  - Stop this iteration
- If ANY reviewer returns BLOCK:
  - Address the blocking issues they identified
  - Do NOT add approval marker
  - Do NOT proceed to next phase
  - The next iteration will re-run this phase's review

## Critical Rules

1. ONE PHASE PER ITERATION - Never start a new phase in the same iteration
2. APPROVAL MARKER = DONE - A phase is only complete when it has "<!-- Phase N: APPROVED -->"
3. CHECKBOXES ≠ APPROVAL - All [x] without approval marker means review still needed
4. REVIEWERS MUST APPROVE - 100% approval required before adding marker
5. NO SKIPPING REVIEWS - Every phase must have its reviewers run
6. REVIEWERS ARE INDEPENDENT - Do not feed them test results or state

## Completion

Output <promise>COMPLETE</promise> ONLY when:
- All phases through Final Phase have approval markers
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
1. Read checklist, find first phase WITHOUT "<!-- Phase N: APPROVED -->" marker
2. If unchecked tasks exist: complete them, mark [x], run tests
3. Spawn phase reviewers IN PARALLEL (from Appendix prompts)
4. If ALL APPROVE: add "<!-- Phase N: APPROVED -->" marker, stop iteration
5. If ANY BLOCK: fix issues, do NOT add marker, next iteration retries

CRITICAL: Checkboxes [x] ≠ phase complete. Only approval marker = complete.

Output <promise>COMPLETE</promise> only when:
- All phases have approval markers
- cargo test --workspace passes

Never skip reviews. Never proceed without approval marker.
```

---

## Notes

- **Iteration limit**: 50 should be enough for 8 phases with some retries
- **Cost estimate**: ~$20-50 depending on how many review cycles needed
- **Monitor**: Check CHECKLIST-COMMS.md for `<!-- Phase N: APPROVED -->` markers
- **Cancel**: Use `/ralph-loop:cancel-ralph` if it goes off track

## Approval Marker Format

After Phase N reviewers approve, add this line immediately after the phase header:

```markdown
## Phase N: [Name]
<!-- Phase N: APPROVED -->
```

This marker is the source of truth for phase completion, not the checkboxes.
