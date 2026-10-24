---- MODULE durable_tail_classification_audit ----
\* Hand-written audit of the durable-tail classification arms of the
\* generated SessionDocumentMachine model.
\*
\* Recovery classifies a session's durable tail from eight observations: the
\* store's durable head relation, the run-id cardinality of the tail (no run
\* rows at all is its own value), its terminal stop reason, its dangling
\* tool-use and orphan tool-result counts, whether messages follow the
\* terminal, what its run cursor says against the transcript, and whether the
\* cursor's bound inputs are all still bound to the run.
\* ClassifyDurableTailCompleted, ClassifyDurableTailRepairable and
\* ClassifyDurableTailContinuable need a verified descendant head (strict, or
\* a store-proven rewrite descendant) and an attributed tail: exactly one run
\* id, or no run rows with an Absent stop on a strict continuation (the
\* store-issued provisional binding) or with an agreeing cursor (either
\* relation). ClassifyDurableTailAmbiguous is their exact complement. The DSL
\* declares state invariants only, and the generated Next is a disjunction,
\* so machine-verify never checks that the arms partition the observations.
\* This audit checks the generated TLA guards, independently of the Rust
\* tables.
\*
\* The four arms are verdicts: each keeps phase Ready, changes only the step
\* count, and its guard reads no state but phase. The runner checks that
\* premise on the generated model before it runs TLC and fails if it stops
\* holding. So the initial state (phase Ready) stands for every reachable
\* state, and the audit checks it alone, with ENABLED over every tuple of the
\* observation product: 6 relations x 4 run-id cardinalities x 4 stop reasons
\* x dangling 0..2 x orphans 0..2 x messages after the terminal x 3 cursor
\* observations x bound inputs, 10368 tuples. Invariants:
\*
\*   AuditClassificationIsTotalAndDisjoint (C1): exactly one arm is enabled
\*     for every tuple.
\*   AuditRewriteClassifiesLikeStrict (C2): the rewrite relation enables
\*     exactly the arm the strict relation enables, for every other value,
\*     except a tail with no run rows and no agreeing cursor: the store
\*     binding attributes it on a strict continuation only.
\*   AuditRewriteUnattributedHolds (C3): the rewrite relation with no run id,
\*     several, or no run rows without an agreeing cursor, enables only
\*     Ambiguous, for every other value.
\*   AuditRewriteEndTurnCompletes (C4): the coherent single-run rewrite tail
\*     that ends at EndTurn with no cursor enables exactly Completed.
\*   AuditRewriteToolUseRepairs: the same tail ending at ToolUse with no
\*     cursor enables exactly Repairable.
\*   AuditAgreeingBoundCursorContinues: the same ToolUse tail with an
\*     agreeing cursor whose inputs are bound enables exactly Continuable, and
\*     so does a strict tail with no run rows and that cursor, so every arm
\*     answers some tuple.
\*   AuditStrictNoRunRowsRepairs: a strict tail with no run rows and no
\*     cursor (a run's closing save before any assistant row) enables exactly
\*     Repairable.
\*   AuditDisagreeingCursorHolds: a cursor that disagrees enables only
\*     Ambiguous.
\*   AuditCursorOnEndTurnHolds: a cursor on an EndTurn tail enables only
\*     Ambiguous (a run's final save clears it).
\*   AuditContinuableNeedsAgreeingBoundCursor: Continuable is enabled only
\*     with an agreeing cursor whose inputs are all bound.
\*   AuditCursorNeverUpgradesAmbiguous: a tuple that is Ambiguous without a
\*     cursor stays Ambiguous with any cursor, except the one designed case:
\*     an agreeing cursor attributes a rewrite tail with no run rows.
\*
\* AuditWitnessReady is the reachability goal the runner checks as an
\* expected violation: the initial state is Ready, so no invariant holds
\* vacuously. The runner's --mutants mode seeds defects into a copy of the
\* generated model and requires TLC to refuse each with the invariant it
\* targets, and records every invariant each defect breaks (TLC -continue)
\* together with the tuples AuditReportGaps names.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* durable_tail_classification_audit.sh:
\*   specs/machines/session_document/durable_tail_classification_audit.sh --mutants
EXTENDS model

AuditSession == CHOOSE s \in SessionIdValues : TRUE
AuditCandidate == CHOOSE c \in RecoveryCandidateIdValues : TRUE
AuditCounts == 0..2
AuditArms == {"Completed", "Repairable", "Continuable", "Ambiguous"}

\* The arms whose generated action is enabled for one observation tuple.
ClassArms(rel, card, stop, d, o, after, cur, bound) ==
    { arm \in AuditArms :
        IF arm = "Completed"
        THEN ENABLED ClassifyDurableTailCompleted(AuditSession, AuditCandidate, rel, card, stop, d, o, after, cur, bound)
        ELSE IF arm = "Repairable"
        THEN ENABLED ClassifyDurableTailRepairable(AuditSession, AuditCandidate, rel, card, stop, d, o, after, cur, bound)
        ELSE IF arm = "Continuable"
        THEN ENABLED ClassifyDurableTailContinuable(AuditSession, AuditCandidate, rel, card, stop, d, o, after, cur, bound)
        ELSE ENABLED ClassifyDurableTailAmbiguous(AuditSession, AuditCandidate, rel, card, stop, d, o, after, cur, bound) }

Tuples ==
    DurableHeadRelationValues \X RunIdCardinalityValues \X DurableTailStopReasonValues
        \X AuditCounts \X AuditCounts \X BOOLEAN \X DurableTailRunCursorValues \X BOOLEAN

Arms(t) == ClassArms(t[1], t[2], t[3], t[4], t[5], t[6], t[7], t[8])

AuditClassificationIsTotalAndDisjoint ==
    phase = "Ready" => \A t \in Tuples : Cardinality(Arms(t)) = 1

AuditRewriteClassifiesLikeStrict ==
    phase = "Ready" =>
        \A card \in RunIdCardinalityValues, stop \in DurableTailStopReasonValues,
           d \in AuditCounts, o \in AuditCounts, after \in BOOLEAN,
           cur \in DurableTailRunCursorValues, bound \in BOOLEAN :
            (card # "NoRunRows" \/ cur = "Agrees") =>
                ClassArms("VerifiedRewriteDescendant", card, stop, d, o, after, cur, bound)
                    = ClassArms("VerifiedStrictDescendant", card, stop, d, o, after, cur, bound)

AuditRewriteUnattributedHolds ==
    phase = "Ready" =>
        \A card \in RunIdCardinalityValues \ {"SingleRunId"},
           stop \in DurableTailStopReasonValues,
           d \in AuditCounts, o \in AuditCounts, after \in BOOLEAN,
           cur \in DurableTailRunCursorValues, bound \in BOOLEAN :
            (card # "NoRunRows" \/ cur # "Agrees") =>
                ClassArms("VerifiedRewriteDescendant", card, stop, d, o, after, cur, bound) = {"Ambiguous"}

AuditRewriteEndTurnCompletes ==
    phase = "Ready" =>
        ClassArms("VerifiedRewriteDescendant", "SingleRunId", "EndTurn", 0, 0, FALSE, "Absent", FALSE)
            = {"Completed"}

AuditRewriteToolUseRepairs ==
    phase = "Ready" =>
        ClassArms("VerifiedRewriteDescendant", "SingleRunId", "ToolUse", 0, 0, FALSE, "Absent", FALSE)
            = {"Repairable"}

AuditAgreeingBoundCursorContinues ==
    phase = "Ready" =>
        /\ ClassArms("VerifiedRewriteDescendant", "SingleRunId", "ToolUse", 0, 0, FALSE, "Agrees", TRUE)
               = {"Continuable"}
        /\ ClassArms("VerifiedStrictDescendant", "NoRunRows", "Absent", 0, 0, FALSE, "Agrees", TRUE)
               = {"Continuable"}

AuditStrictNoRunRowsRepairs ==
    phase = "Ready" =>
        ClassArms("VerifiedStrictDescendant", "NoRunRows", "Absent", 0, 0, FALSE, "Absent", FALSE)
            = {"Repairable"}

AuditDisagreeingCursorHolds ==
    phase = "Ready" => \A t \in Tuples : t[7] = "Disagrees" => Arms(t) = {"Ambiguous"}

AuditCursorOnEndTurnHolds ==
    phase = "Ready" =>
        \A t \in Tuples : (t[3] = "EndTurn" /\ t[7] # "Absent") => Arms(t) = {"Ambiguous"}

AuditContinuableNeedsAgreeingBoundCursor ==
    phase = "Ready" =>
        \A t \in Tuples : "Continuable" \in Arms(t) => (t[7] = "Agrees" /\ t[8] = TRUE)

AuditCursorNeverUpgradesAmbiguous ==
    phase = "Ready" =>
        \A t \in Tuples :
            (/\ ClassArms(t[1], t[2], t[3], t[4], t[5], t[6], "Absent", FALSE) = {"Ambiguous"}
             /\ ~(t[1] = "VerifiedRewriteDescendant" /\ t[2] = "NoRunRows" /\ t[7] = "Agrees"))
                => Arms(t) = {"Ambiguous"}

\* Mutant runs only: names how many tuples enable no arm or several, with one
\* example of each, so a defect's gap is visible in its log. Always true.
AuditReportGaps ==
    LET none == {t \in Tuples : Arms(t) = {}}
        many == {t \in Tuples : Cardinality(Arms(t)) > 1}
    IN /\ PrintT(<<"tuples with no arm", Cardinality(none),
                   IF none = {} THEN "none" ELSE CHOOSE t \in none : TRUE>>)
       /\ PrintT(<<"tuples with several arms", Cardinality(many),
                   IF many = {} THEN "none" ELSE CHOOSE t \in many : TRUE>>)

\* The audit checks the initial state alone; the premise above makes it
\* stand for every reachable state.
AuditSpec == Init /\ [][FALSE]_vars

\* The reachability goal, checked as an expected violation.
AuditWitnessReady == phase = "Ready"
NotAuditWitnessReady == ~AuditWitnessReady
====
